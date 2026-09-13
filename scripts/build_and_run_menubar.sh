#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PROJECT_PATH="$ROOT_DIR/macos/ClipmemMenuBar/ClipmemMenuBar.xcodeproj"
DERIVED_DATA="$ROOT_DIR/macos/ClipmemMenuBar/DerivedData"
CLIPMEM_BIN="$ROOT_DIR/target/debug/clipmem"
APP_NAME="ClipmemMenuBar"
DEFAULT_DB_PATH="$DERIVED_DATA/clipmem-dev.sqlite3"
DB_PATH="$DEFAULT_DB_PATH"
START_WATCHER=1
WATCHER_PID_FILE="$DERIVED_DATA/clipmem-watch.pid"
WATCHER_STDOUT="$DERIVED_DATA/clipmem-watch.stdout.log"
WATCHER_STDERR="$DERIVED_DATA/clipmem-watch.stderr.log"
WATCHER_PLIST="$DERIVED_DATA/io.openclaw.clipmem.watch.dev.plist"
WATCHER_LABEL="io.openclaw.clipmem.watch.dev"

usage() {
  cat <<'USAGE'
Usage: scripts/build_and_run_menubar.sh [--app-only] [--db PATH]

Builds the debug CLI and menu bar app with a separate development archive
and watcher. Installed production watchers are left running.

Options:
  --app-only   Launch only the app and leave watcher state untouched.
  --db PATH    Use PATH as the watcher/app database.
USAGE
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --app-only)
      START_WATCHER=0
      shift
      ;;
    --db)
      if [[ $# -lt 2 ]]; then
        echo "--db requires a path" >&2
        exit 2
      fi
      DB_PATH="$2"
      shift 2
      ;;
    --help|-h)
      usage
      exit 0
      ;;
    *)
      echo "Unknown argument: $1" >&2
      usage >&2
      exit 2
      ;;
  esac
done

stop_existing_app() {
  local pids

  pids="$(pgrep -x "$APP_NAME" || true)"
  if [[ -z "$pids" ]]; then
    return
  fi

  echo "Stopping existing $APP_NAME instances..."
  osascript -e 'tell application id "io.openclaw.clipmem.menubar" to quit' >/dev/null 2>&1 || true

  local deadline=$((SECONDS + 5))
  while [[ $SECONDS -lt $deadline ]]; do
    if ! pgrep -x "$APP_NAME" >/dev/null; then
      return
    fi
    sleep 0.2
  done

  pkill -x "$APP_NAME" || true
  sleep 0.5
}

running_app_paths() {
  local pids

  pids="$(pgrep -x "$APP_NAME" | paste -sd, - || true)"
  if [[ -z "$pids" ]]; then
    return
  fi

  ps -ww -p "$pids" -o command= | sed 's#/Contents/MacOS/ClipmemMenuBar$##'
}

stop_dev_watcher() {
  local uid

  uid="$(id -u)"
  launchctl bootout "gui/$uid/$WATCHER_LABEL" >/dev/null 2>&1 || true

  if [[ ! -f "$WATCHER_PID_FILE" ]]; then
    return
  fi

  local pid
  pid="$(cat "$WATCHER_PID_FILE" 2>/dev/null || true)"
  rm -f "$WATCHER_PID_FILE"
  if [[ -z "$pid" ]]; then
    return
  fi

  if kill -0 "$pid" >/dev/null 2>&1; then
    echo "Stopping previous dev watcher pid $pid..."
    kill "$pid" >/dev/null 2>&1 || true
    local deadline=$((SECONDS + 5))
    while [[ $SECONDS -lt $deadline ]]; do
      if ! kill -0 "$pid" >/dev/null 2>&1; then
        return
      fi
      sleep 0.2
    done
    kill -9 "$pid" >/dev/null 2>&1 || true
  fi
}

xml_escape() {
  local value="$1"
  value="${value//&/&amp;}"
  value="${value//</&lt;}"
  value="${value//>/&gt;}"
  value="${value//\"/&quot;}"
  value="${value//\'/&apos;}"
  printf '%s' "$value"
}

start_dev_watcher() {
  local uid
  local pid

  uid="$(id -u)"
  mkdir -p "$DERIVED_DATA"
  : >"$WATCHER_STDOUT"
  : >"$WATCHER_STDERR"
  cat >"$WATCHER_PLIST" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
  <dict>
    <key>Label</key>
    <string>$WATCHER_LABEL</string>
    <key>ProgramArguments</key>
    <array>
      <string>$(xml_escape "$CLIPMEM_BIN")</string>
      <string>watch</string>
      <string>--skip-initial</string>
      <string>--db</string>
      <string>$(xml_escape "$DB_PATH")</string>
      <string>--interval-ms</string>
      <string>350</string>
    </array>
    <key>RunAtLoad</key>
    <true/>
    <key>KeepAlive</key>
    <true/>
    <key>StandardOutPath</key>
    <string>$(xml_escape "$WATCHER_STDOUT")</string>
    <key>StandardErrorPath</key>
    <string>$(xml_escape "$WATCHER_STDERR")</string>
  </dict>
</plist>
PLIST

  echo "Starting dev watcher..."
  launchctl enable "gui/$uid/$WATCHER_LABEL" >/dev/null 2>&1 || true
  launchctl bootstrap "gui/$uid" "$WATCHER_PLIST"
  launchctl kickstart -k "gui/$uid/$WATCHER_LABEL"
  sleep 0.5
  pid="$(launchctl list | awk -v label="$WATCHER_LABEL" '$3 == label { print $1 }')"
  if [[ -z "$pid" || "$pid" == "-" ]]; then
    echo "Dev watcher failed to start. stderr:" >&2
    tail -n 40 "$WATCHER_STDERR" >&2 || true
    exit 1
  fi
  echo "$pid" >"$WATCHER_PID_FILE"
  echo "Dev watcher running: pid=$pid binary=$CLIPMEM_BIN database=$DB_PATH"
  echo "Dev watcher logs: $WATCHER_STDOUT $WATCHER_STDERR"
}

echo "Building Rust backend..."
cargo build --manifest-path "$ROOT_DIR/Cargo.toml"
python3 "$ROOT_DIR/scripts/check_version_sync.py"

if [[ "$START_WATCHER" == "1" ]]; then
  stop_dev_watcher
  start_dev_watcher
else
  echo "App-only mode: watcher state was left untouched."
fi

stop_existing_app

echo "Building macOS menu bar app..."
xcodebuild \
  -project "$PROJECT_PATH" \
  -scheme ClipmemMenuBar \
  -configuration Debug \
  -derivedDataPath "$DERIVED_DATA" \
  build

APP_PATH="$DERIVED_DATA/Build/Products/Debug/ClipmemMenuBar.app"
if [[ ! -d "$APP_PATH" ]]; then
  echo "Built app was not found at $APP_PATH" >&2
  exit 1
fi

echo "Launching $APP_PATH"
open -n --env "CLIPMEM_BINARY_PATH=$CLIPMEM_BIN" --env "CLIPMEM_DB_PATH=$DB_PATH" "$APP_PATH"
sleep 2

if ! running_app_paths | grep -Fxq "$APP_PATH"; then
  echo "Failed to verify $APP_PATH is running." >&2
  exit 1
fi

echo "$APP_NAME launched with CLIPMEM_BINARY_PATH=$CLIPMEM_BIN"
echo "$APP_NAME database path: $DB_PATH"
