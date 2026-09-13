import Foundation
import Darwin

struct CommandTimeoutError: Error, LocalizedError, Equatable, Sendable {
    let commandCategory: String
    let deadline: String

    var errorDescription: String? {
        "The \(commandCategory) command exceeded its \(deadline) deadline."
    }
}

struct CommandResult: Sendable {
    var exitCode: Int32
    var stdout: Data
    var stderr: Data

    var stdoutText: String {
        String(data: stdout, encoding: .utf8) ?? ""
    }

    var stderrText: String {
        String(data: stderr, encoding: .utf8) ?? ""
    }
}

struct CommandRunner: Sendable {
    private let processStarted: (@Sendable () -> Void)?

    init(processStarted: (@Sendable () -> Void)? = nil) {
        self.processStarted = processStarted
    }

    static func defaultTimeout(arguments: [String]) -> Duration {
        let command = arguments.dropFirst(arguments.first == "--db" ? 2 : 0)
        let maintenance = ["setup", "storage", "ocr", "purge"].contains(command.first ?? "")
            || (command.first == "settings" && command.dropFirst().first == "retention")
        return .seconds(maintenance ? 1800 : 60)
    }

    func run(executable: String, arguments: [String]) async throws -> CommandResult {
        try await run(executable: executable, arguments: arguments, timeout: nil)
    }

    func run(executable: String, arguments: [String], timeout: Duration?) async throws -> CommandResult {
        let timeout: Duration? = timeout ?? Self.defaultTimeout(arguments: arguments)
        let runningProcess = RunningProcess()
        let cancellationState = CancellationState()
        let timeoutError = timeout.map {
            CommandTimeoutError(
                commandCategory: URL(fileURLWithPath: executable).lastPathComponent,
                deadline: String(describing: $0)
            )
        }
        let processStarted = processStarted
        return try await withTaskCancellationHandler {
            let commandTask = Task.detached(priority: .userInitiated) {
                let process = Process()
                let stdout = Pipe()
                let stderr = Pipe()
                let stdoutReader = PipeReader(fileHandle: stdout.fileHandleForReading)
                let stderrReader = PipeReader(fileHandle: stderr.fileHandleForReading)

                process.executableURL = URL(fileURLWithPath: executable)
                process.arguments = arguments
                process.standardOutput = stdout
                process.standardError = stderr
                runningProcess.set(process, readers: [stdoutReader, stderrReader])
                defer { runningProcess.clear() }

                stdoutReader.start()
                stderrReader.start()
                do {
                    try cancellationState.checkCancellation()
                } catch {
                    stdout.fileHandleForWriting.closeFile()
                    stderr.fileHandleForWriting.closeFile()
                    _ = try? stdoutReader.wait()
                    _ = try? stderrReader.wait()
                    throw error
                }

                do {
                    try process.run()
                    try cancellationState.checkCancellation()
                } catch {
                    runningProcess.terminateAndEscalate()
                    if process.isRunning { process.waitUntilExit() }
                    stdout.fileHandleForWriting.closeFile()
                    stderr.fileHandleForWriting.closeFile()
                    _ = try? stdoutReader.wait()
                    _ = try? stderrReader.wait()
                    throw error
                }

                processStarted?()
                process.waitUntilExit()
                let stdoutData = try stdoutReader.wait()
                let stderrData = try stderrReader.wait()
                try cancellationState.checkCancellation()
                return CommandResult(exitCode: process.terminationStatus, stdout: stdoutData, stderr: stderrData)
            }
            if let timeout {
                let timeoutTask = Task {
                    try? await Task.sleep(for: timeout)
                    if !Task.isCancelled {
                        cancellationState.timeout(timeoutError!)
                        runningProcess.terminateAndEscalate()
                    }
                }
                defer { timeoutTask.cancel() }
                return try await commandTask.value
            }
            return try await commandTask.value
        } onCancel: {
            cancellationState.cancel()
            runningProcess.terminateAndEscalate()
        }
    }

    func runStreaming(
        executable: String,
        arguments: [String],
        timeout: Duration = .seconds(1800),
        onStdoutLine: @escaping @Sendable (String) async throws -> Void
    ) async throws -> CommandResult {
        let runningProcess = RunningProcess()
        let cancellationState = CancellationState()
        let timeoutTask = Task {
            try? await Task.sleep(for: timeout)
            if !Task.isCancelled {
                cancellationState.timeout(CommandTimeoutError(commandCategory: URL(fileURLWithPath: executable).lastPathComponent, deadline: String(describing: timeout)))
                runningProcess.terminateAndEscalate()
            }
        }
        defer { timeoutTask.cancel() }
        let processStarted = processStarted
        return try await withTaskCancellationHandler {
            try await Task.detached(priority: .userInitiated) {
                let process = Process()
                let stdout = Pipe()
                let stderr = Pipe()
                let stderrReader = PipeReader(fileHandle: stderr.fileHandleForReading)

                process.executableURL = URL(fileURLWithPath: executable)
                process.arguments = arguments
                process.standardOutput = stdout
                process.standardError = stderr
                runningProcess.set(process, readers: [stderrReader])
                defer { runningProcess.clear() }
                defer { try? stdout.fileHandleForReading.close() }

                stderrReader.start()
                do {
                    try cancellationState.checkCancellation()
                    try process.run()
                    processStarted?()
                    let stdoutData = try await Self.consumeStdout(
                        from: stdout.fileHandleForReading,
                        cancellationState: cancellationState,
                        onStdoutLine: onStdoutLine
                    )
                    process.waitUntilExit()
                    let stderrData = try stderrReader.wait()
                    try cancellationState.checkCancellation()
                    return CommandResult(exitCode: process.terminationStatus, stdout: stdoutData, stderr: stderrData)
                } catch {
                    runningProcess.terminateAndEscalate()
                    stdout.fileHandleForWriting.closeFile()
                    stderr.fileHandleForWriting.closeFile()
                    stderrReader.close()
                    if process.isRunning {
                        process.waitUntilExit()
                    }
                    throw error
                }
            }.value
        } onCancel: {
            cancellationState.cancel()
            runningProcess.terminateAndEscalate()
        }
    }

    private static func consumeStdout(
        from fileHandle: FileHandle,
        cancellationState: CancellationState,
        onStdoutLine: @escaping @Sendable (String) async throws -> Void
    ) async throws -> Data {
        var output = Data()
        var pending = Data()
        let descriptor = fileHandle.fileDescriptor
        let flags = fcntl(descriptor, F_GETFL)
        guard flags >= 0, fcntl(descriptor, F_SETFL, flags | O_NONBLOCK) >= 0 else {
            throw POSIXError(POSIXErrorCode(rawValue: errno) ?? .EIO)
        }
        var buffer = [UInt8](repeating: 0, count: 65536)

        while true {
            try cancellationState.checkCancellation()
            var state = pollfd(fd: descriptor, events: Int16(POLLIN | POLLHUP), revents: 0)
            let ready = poll(&state, 1, 100)
            if ready == 0 { continue }
            if ready < 0 {
                if errno == EINTR { continue }
                throw POSIXError(POSIXErrorCode(rawValue: errno) ?? .EIO)
            }
            let count = read(descriptor, &buffer, buffer.count)
            if count == 0 { break }
            if count < 0 {
                if errno == EAGAIN || errno == EINTR { continue }
                throw POSIXError(POSIXErrorCode(rawValue: errno) ?? .EIO)
            }
            let chunk = Data(buffer.prefix(count))
            output.append(chunk)
            pending.append(chunk)

            while let newline = pending.firstIndex(of: 0x0A) {
                let lineData = pending[..<newline]
                pending.removeSubrange(...newline)
                guard let line = String(data: lineData, encoding: .utf8) else {
                    throw ClipmemClientError.decodingFailed("Could not decode clipmem progress output.")
                }
                if !line.isEmpty {
                    try await onStdoutLine(line)
                }
            }
        }

        if !pending.isEmpty {
            guard let line = String(data: pending, encoding: .utf8) else {
                throw ClipmemClientError.decodingFailed("Could not decode clipmem progress output.")
            }
            try await onStdoutLine(line)
        }

        return output
    }
}

// Accessed by a cancellation handler and a worker task, so access is synchronized.
private final class RunningProcess: @unchecked Sendable {
    private let lock = NSLock()
    private var process: Process?
    private var readers: [PipeReader] = []

    func set(_ process: Process, readers: [PipeReader] = []) {
        lock.lock()
        self.process = process
        self.readers = readers
        lock.unlock()
    }

    func terminateAndEscalate() {
        lock.lock()
        let process = process
        let readers = readers
        lock.unlock()
        for reader in readers { reader.close() }
        guard let process, process.isRunning else { return }
        process.terminate()
        let pid = process.processIdentifier
        DispatchQueue.global(qos: .userInitiated).asyncAfter(deadline: .now() + .milliseconds(500)) {
            if process.isRunning {
                kill(pid, SIGKILL)
            }
        }
    }

    func clear() {
        lock.lock()
        process = nil
        readers = []
        lock.unlock()
    }
}

private final class CancellationState: @unchecked Sendable {
    private let lock = NSLock()
    private var error: (any Error & Sendable)?

    func cancel() {
        lock.lock()
        if error == nil { error = CancellationError() }
        lock.unlock()
    }

    func timeout(_ timeoutError: CommandTimeoutError) {
        lock.lock()
        if error == nil { error = timeoutError }
        lock.unlock()
    }

    func checkCancellation() throws {
        lock.lock()
        let error = error
        lock.unlock()
        if let error { throw error }
    }
}

private final class PipeReader: @unchecked Sendable {
    private let fileHandle: FileHandle
    private let semaphore = DispatchSemaphore(value: 0)
    private let lock = NSLock()
    private var data = Data()
    private var isClosed = false
    private var failure: POSIXError?

    init(fileHandle: FileHandle) {
        self.fileHandle = fileHandle
    }

    func start() {
        DispatchQueue.global(qos: .userInitiated).async {
            defer {
                try? self.fileHandle.close()
                self.semaphore.signal()
            }
            let descriptor = self.fileHandle.fileDescriptor
            let flags = fcntl(descriptor, F_GETFL)
            guard flags >= 0, fcntl(descriptor, F_SETFL, flags | O_NONBLOCK) >= 0 else { self.recordFailure(); return }
            var buffer = [UInt8](repeating: 0, count: 65536)
            while true {
                self.lock.lock()
                let closed = self.isClosed
                self.lock.unlock()
                if closed { return }
                var descriptorState = pollfd(fd: descriptor, events: Int16(POLLIN | POLLHUP), revents: 0)
                let ready = poll(&descriptorState, 1, 100)
                if ready == 0 { continue }
                if ready < 0 {
                    if errno == EINTR { continue }
                    self.recordFailure()
                    return
                }
                let count = read(descriptor, &buffer, buffer.count)
                if count == 0 { return }
                if count < 0 {
                    if errno == EAGAIN || errno == EINTR { continue }
                    self.recordFailure()
                    return
                }
                self.lock.lock()
                self.data.append(contentsOf: buffer.prefix(count))
                self.lock.unlock()
            }
        }
    }

    private func recordFailure() {
        let error = POSIXError(POSIXErrorCode(rawValue: errno) ?? .EIO)
        lock.lock()
        failure = error
        lock.unlock()
    }

    func wait() throws -> Data {
        semaphore.wait()
        lock.lock()
        let output = data
        let failure = failure
        lock.unlock()
        if let failure { throw failure }
        return output
    }

    func close() {
        lock.lock()
        guard isClosed == false else {
            lock.unlock()
            return
        }
        isClosed = true
        lock.unlock()
    }
}
