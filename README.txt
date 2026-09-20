PIC NFS grid / thumbnail-first source replacements

Files in this ZIP:
  src/db/core.rs
  src/window/library.rs
  src/scanner.rs

No NFS transport, share registration, dialogs or SMB files are included.
The changes add a read-only grid DB opener and sort NFS thumbnail work to
process smaller non-RAW images before RAW files. Full RAW downloads are NOT
solved by these changes. Files are from the last full-source snapshot,
which may differ from your present checkout. Make a backup first.

From your project root after extracting this ZIP into a temporary directory:
  mkdir -p ~/pic-before-performance/src/{db,window}
  cp src/db/core.rs ~/pic-before-performance/src/db/
  cp src/window/library.rs ~/pic-before-performance/src/window/
  cp src/scanner.rs ~/pic-before-performance/src/
  cp <extracted>/src/db/core.rs src/db/core.rs
  cp <extracted>/src/window/library.rs src/window/library.rs
  cp <extracted>/src/scanner.rs src/scanner.rs
  cargo check

Restore from ~/pic-before-performance/src/ if necessary.
