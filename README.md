# macmemana

<p align="center">
  <img src="statics/MACMEMANALOGO.svg#gh-light-mode-only" alt="macmemana logo" width="600"/>
  <img src="statics/MACMEMANALOGO_dark.svg#gh-dark-mode-only" alt="macmemana logo" width="600"/>
</p>

**macmemana** (Mac Memory Analyzer) is a terminal-based memory analysis tool specifically designed for macOS. It provides accurate swap usage reporting by leveraging `vmmap` to dig into process memory details, solving common discrepancies found in standard tools.

## Features

- **Accurate Swap Accounting**: Uses `vmmap` to distinguish between compressed memory, resident memory, and actual disk swap, providing a more realistic view of swap usage per process.
- **System Swap Normalization**: Automatically adjusts process swap estimates to match the system-wide swap usage reported by the kernel.
- **TUI Interface**: Interactive terminal user interface built with `ratatui` for real-time monitoring and sorting.
- **CLI Mode**: Support for one-shot output for scripting or quick checks.
- **Process Management**: Ability to kill processes directly from the TUI.

## Installation

Ensure you have Rust and Cargo installed.

```bash
cargo install --path .
# Or build manually
cargo build --release
```

## Usage

### TUI Mode (Interactive)

Running with `sudo` is highly recommended (and often required) to allow `vmmap` to inspect other users' processes and system services like `WindowServer`.

```bash
sudo macmemana
```

### CLI Mode

Dump the process list sorted by swap usage:

```bash
sudo macmemana --cli --sort swap
```

Available sort options: `swap`, `phys` (physical footprint).

## Keyboard Shortcuts

| Key | Action |
| --- | --- |
| `q` | Quit application |
| `r` | Refresh (trigger a new scan) |
| `j` / `↓` | Select next process |
| `k` / `↑` | Select previous process |
| `s` | Sort by **Swap** (Descending) |
| `p` | Sort by **Physical** Memory (Descending) |
| `c` | Sort by **Compressed** Memory (Descending) |
| `t` | Sort by **Total** Memory (Descending) |
| `n` | Sort by **Name** (Ascending) |
| `i` | Sort by **PID** (Ascending) |
| `x` | Kill selected process |

## Why sudo?

macOS restricts access to memory maps of processes owned by other users or the system. Without `sudo`, `macmemana` can only analyze your own processes, leading to incomplete system-wide swap statistics.

## License

MIT

