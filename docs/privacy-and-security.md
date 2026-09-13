# Privacy and security

clipmem is local-only by design. All clipboard data stays on your
machine, and there are multiple layers of control over what gets
captured and how long it's kept.

## Local clipboard data, no telemetry

Clipboard capture, search, restore, and OCR run locally without telemetry.
The menu bar app checks GitHub for stable releases at startup and every
12 hours, with a one-hour retry delay after failures. The CLI's explicit
`app update-check run` command also contacts GitHub. These requests contain
no clipboard contents. Agent integration commands can invoke an installed
agent CLI; any agent you give clipboard access to follows its own provider
and data-sharing settings.

When OCR is enabled, image text recognition runs locally through Apple
Vision on macOS. clipmem doesn't send images or OCR text to any remote
service.

## Database location

The database lives at:

```
~/Library/Application Support/clipmem/clipmem.sqlite3
```

Logs are in the same directory under `logs/`.

## File permissions

clipmem enforces strict file permissions at runtime:

- Newly created archive directory: `0700` (owner read/write/execute only). Existing custom parent directories retain their permissions.
- Database and logs: `0600` (owner read/write only)

These permissions are enforced by the managed service setup flow and
checked at runtime in `src/db/core.rs::harden_path_permissions`.

## Encryption

The database is not encrypted. Rely on FileVault (or similar
full-disk encryption) for at-rest protection.

## Blob storage

Blob payloads (images, PDFs, RTF, file URLs, and other clipboard
representations) retain their captured bytes so `clipmem restore` can
faithfully rehydrate them. Image preview generation stores a separate bounded
lossless WebP derivative and never replaces the source image. PDFs are not
previewed by this maintenance command.

This means the database can grow if you frequently copy large binary content.
Use `clipmem storage optimize-images` to prepare efficient previews and
`clipmem storage compact` after purging history when you want to reclaim free
SQLite pages. Preview generation does not claim source-storage savings.

## Controlling what gets captured

You have several options to limit what clipmem archives:

### Pause capture

Persistently pause all capture without stopping the watcher service:

```bash
clipmem settings pause on
clipmem settings pause off
```

### API key filter

Opt into a precision-biased secret guard that skips clipboard
snapshots matching API key or token patterns:

```bash
clipmem settings api-key-filter on
```

Matching snapshots are skipped entirely instead of being redacted —
no preview text, search text, or blob payloads are written. The filter
is heuristic and tuned to avoid false positives rather than catch every
possible secret.

### OCR

Opt into local OCR for image captures:

```bash
clipmem settings ocr on
clipmem settings ocr off
```

OCR is disabled by default. When enabled, new image captures are stored
immediately and OCR runs afterward. OCR failures don't block capture.
Completed OCR text and status are stored in SQLite and become
searchable by `search`, `recall`, `recent`, and `timeline`.

To process existing image snapshots, run:

```bash
clipmem ocr run
clipmem ocr status
```

Use the same privacy model for OCR text as for copied plain text:
any readable text inside copied images can become searchable in the
local database.

### Ignore list

Exclude specific apps from capture by their bundle ID:

```bash
clipmem settings ignore add com.apple.Passwords
clipmem settings ignore list
```

Matching is exact and case-insensitive.

### Retention

Automatically prune old snapshots after a set duration:

```bash
clipmem settings retention 30d
clipmem settings retention forever
```

Or manually prune on demand:

```bash
clipmem purge --older-than 30d
```

## Uninstall and full cleanup

To completely remove clipmem and all its data:

1. Stop and remove the managed background service:

```bash
clipmem service uninstall
```

2. Remove installed agent skills if you installed them:

```bash
clipmem agents openclaw uninstall-skill
clipmem agents hermes uninstall-skill
```

3. Delete the database and logs:

```bash
rm -rf ~/Library/Application\ Support/clipmem
```

4. Remove the binary:

```bash
# If installed via Cargo
cargo uninstall clipmem

# If installed manually
rm -f ~/.local/bin/clipmem

# If installed via Homebrew
brew uninstall clipmem
```

5. If you installed the menu bar app via Homebrew:

```bash
brew uninstall --cask clipmem-app
```

## Next steps

- [Managing your archive](managing-your-archive.md) — restore, delete,
  and configure capture policy
- [Architecture](architecture.md) — understand how clipmem stores and
  indexes your data
