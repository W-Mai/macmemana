# Changelog

All notable changes to this project will be documented in this file.

## [0.3.1] - 2026-01-26

### Fixed
- **UI**: Fixed spinner animation logic to ensure smooth rotation.

## [0.3.0] - 2026-01-26

### Added
- **Detail View**: Press `Enter` on a process to view detailed information including:
  - Command line arguments
  - Execution path and CWD
  - CPU usage and start time
  - Detailed memory breakdown (Physical, Compressed, Swap, Total)
- **Single Process Refresh**: Press `Shift+R` (or `R`) to refresh only the selected process, reducing system overhead.
- **Controls Reference**: Added `Enter` and `Shift+R` to the controls panel.

### Changed
- **UI Layout**: Optimized status bar layout for better visibility of system stats and shortcuts.
- **Internal**: Refactored `App` struct to support detail view state.
- **Internal**: Removed unused dependencies and cleaned up warnings.

### Fixed
- Addressed compiler warnings and clippy lints.
