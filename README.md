# Pochi CLI

Cross-platform command-line tool for Pochi-DIO — an RP2350-based high-speed digital I/O unit.

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

## Linux Setup (udev rules)

Without a rule the serial port belongs to `root:dialout`, ModemManager may probe
it, and the kernel may suspend the USB link. Install the supplied rules once:

```bash
sudo install -m 0644 packaging/udev/99-pochi.rules /etc/udev/rules.d/99-pochi.rules
sudo udevadm control --reload-rules && sudo udevadm trigger
sudo usermod -aG dialout "$USER"      # only for sessions without a seat (SSH, services); log in again
```

Unplug and re-plug the device afterwards. [`packaging/udev/99-pochi.rules`](packaging/udev/99-pochi.rules)
(VID `2e8a`, PID `1175`) does four things:

| Setting | Why |
|---|---|
| `TAG+="uaccess"` | the user logged in at the seat can open `/dev/ttyACM*` without `sudo` |
| `GROUP="dialout"`, `MODE="0660"` | members of `dialout` can open it from SSH or a service (use `uucp` on Arch) |
| `ENV{ID_MM_DEVICE_IGNORE}="1"` | ModemManager does not probe the port with AT commands |
| `ATTR{power/control}="on"` | USB autosuspend is off, so a sleeping link cannot cause command timeouts or trip the watchdog |

A last rule gives `picotool` access to the device in BOOTSEL mode (PID `000f`).

Check the syntax with `udevadm verify packaging/udev/99-pochi.rules`, and check the
result with `udevadm info -a -n /dev/ttyACM0 | grep -E 'idVendor|idProduct'` and
`cat /sys/bus/usb/devices/<bus-port>/power/control` (it prints `on`).

### Windows

Windows needs no driver (the standard `usbser` driver is used) but may suspend the
device through **USB selective suspend** when the port is idle. Turn it off for the
power plan in use: *Control Panel > Power Options > Change plan settings > Change
advanced power settings > USB settings > USB selective suspend setting > Disabled*.
On a laptop set both "On battery" and "Plugged in". If the device still drops out,
open Device Manager, select the Pochi device under *Universal Serial Bus controllers*
(and the USB Root Hub it hangs on), and clear *Allow the computer to turn off this
device to save power* on the *Power Management* tab. Selective suspend does not change
the outputs, but a watchdog (`wdog`) trips when the host stops answering for its
timeout.

## Firmware Update

The firmware is updated over USB only (BOOTSEL, copy the UF2, check `pochi version`).
See `docs/firmware-update.md` in the `pochi-tools` repository (also lists the files that ship with every release and how to verify them).

## Documentation

- **CLI Usage**: See the sections above and `pochi --help` / `pochi <subcommand> --help`
- **Firmware Update**: See [`docs/firmware-update.md`](https://github.com/alphaelements/pochi-tools/blob/main/docs/firmware-update.md) in the pochi-tools repository

## License

Licensed under the [Apache License, Version 2.0](LICENSE).
