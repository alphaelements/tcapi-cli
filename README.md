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

## Safety Functions

```bash
pochi outputs reset              # every output to its default (def); there is no "alloff"
pochi safety status              # watchdog, failsafe latch, usbsafe, exclusive groups
pochi safety apply safety.toml   # excl, usbsafe, then wdog; verified by read-back
pochi safety hold                # feed the watchdog until Ctrl+C (outputs unchanged)
pochi wdog 500                   # 100-10000 ms, 0 / off disables
pochi failsafe clear
pochi excl set 0x0c 0x30         # pins of one group are never ON together
pochi excl get
pochi usbsafe on
pochi rstinfo
```

Every `pochi` run connects and exits, so a watchdog enabled with `wdog` or
`safety apply` **trips the device once that process has ended** and the timeout has
elapsed (outputs go to their defaults, the failsafe latch is set). Keep it fed with
`pochi safety hold`. Settings are volatile: they are off after every restart.

Exit codes: 0 ok, 1 device error, 2 connection error, 3 argument error,
4 `ERR FAILSAFE` (failsafe latch refuses the command), 5 `ERR EXCLUSIVE` (an exclusive
group would be violated; `--json` carries `kind`, `group` and `mask`).

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
