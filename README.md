# Tachyon CLI

Cross-platform command-line tool for [Tachyon DIO](http://gitlab.home:8888/Ryoma.Shiegeyama/tachyon) — an RP2350-based high-speed digital I/O unit.

## Features

- **Single binary** — Rust, cross-compiled for Linux / macOS / Windows
- **CDC text mode** — communicates via Phase 8 human-readable text protocol
- **Multi-device** — enumerate, select by UID / device number / port, detect & resolve duplicates
- **JSON output** — `--json` flag for scripting and MCP server integration
- **TOML configs** — load pin maps and output patterns from TOML files

## Quick Start

```bash
# Install from source
cargo install --path .

# List connected devices
tachyon list

# Read input pins
tachyon read

# Get firmware version
tachyon version

# Send raw text command
tachyon raw "help"
```

## Device Selection

```bash
# Auto-detect (single device)
tachyon version

# By device number
tachyon -d 1 version

# By UID (hardware-unique)
tachyon -u D5B305543EC559BF version

# By port path
tachyon -p /dev/ttyACM1 version
```

## Documentation

- **User Guide**: [Wiki](http://gitlab.home:8888/Ryoma.Shiegeyama/tachyon-cli/-/wikis/home) (日本語 / English)
- **Dev Spec**: [tachyon/docs/55-cli-spec.md](http://gitlab.home:8888/Ryoma.Shiegeyama/tachyon/-/blob/main/docs/55-cli-spec.md)
- **FW Protocol**: [tachyon/docs/30-usb-protocol.md](http://gitlab.home:8888/Ryoma.Shiegeyama/tachyon/-/blob/main/docs/30-usb-protocol.md)

## License

MIT OR Apache-2.0
