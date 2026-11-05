# Pochi CLI

Cross-platform command-line tool for [Pochi-DIO](http://gitlab.home:8888/apro/eda/pochi-dio) — an RP2350-based high-speed digital I/O unit.

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
pochi list

# Read input pins
pochi read

# Get firmware version
pochi version

# Send raw text command
pochi raw "help"
```

## Device Selection

```bash
# Auto-detect (single device)
pochi version

# By device number
pochi -d 1 version

# By UID (hardware-unique)
pochi -u D5B305543EC559BF version

# By port path
pochi -p /dev/ttyACM1 version
```

## Documentation

- **User Guide**: [Wiki](http://gitlab.home:8888/apro/eda/pochi-cli/-/wikis/home) (日本語 / English)
- **Dev Spec**: [pochi-dio/docs/55-cli-spec.md](http://gitlab.home:8888/apro/eda/pochi-dio/-/blob/main/docs/55-cli-spec.md)
- **FW Protocol**: [pochi-dio/docs/30-usb-protocol.md](http://gitlab.home:8888/apro/eda/pochi-dio/-/blob/main/docs/30-usb-protocol.md)

## License

Licensed under the [Apache License, Version 2.0](LICENSE).
