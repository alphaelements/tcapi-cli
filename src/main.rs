use clap::{Parser, Subcommand, ValueEnum};
use std::process;
use tcapi::{commands, device, exit, output, parse};
use tcapi_sdk::TS_GET_MAX;

#[derive(Parser)]
#[command(name = "tcapi", version, about = "CLI tool for Tcapi-DIO")]
struct Cli {
    /// Output in JSON format
    #[arg(long, global = true)]
    json: bool,

    /// Select device by UID (USB serial number)
    #[arg(long, global = true)]
    uid: Option<String>,

    /// Select device by device number
    #[arg(short = 'd', long, global = true)]
    device: Option<u8>,

    /// Select device by serial port path
    #[arg(short = 'p', long, global = true)]
    port: Option<String>,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// List connected Tcapi-DIO devices
    List,

    /// Get firmware version
    Version,

    /// Get device capabilities
    Caps,

    /// Get device info (UID, device number)
    Info,

    /// Set device number (0-254, 'reset', or 'auto')
    Devnum {
        /// Device number (0-254), 'reset', or 'auto'
        value: String,
    },

    /// Pin map operations
    Map {
        #[command(subcommand)]
        action: MapAction,
    },

    /// Read all input values
    Read,

    /// Set output values
    Write {
        /// Output values (hex, e.g. 0x05)
        values: String,
        /// Update mask (hex, e.g. 0x0F)
        mask: String,
    },

    /// Get full I/O state
    State,

    /// Watch input changes in real-time
    Watch,

    /// Pattern operations
    Pattern {
        #[command(subcommand)]
        action: PatternAction,
    },

    /// Timestamp operations
    Ts {
        #[command(subcommand)]
        action: TsAction,
    },

    /// Multi-device sync operations
    Sync {
        #[command(subcommand)]
        action: SyncAction,
    },

    /// Send raw text command
    Raw {
        /// Command text to send
        text: String,
    },

    /// Soft-reset the device (USB re-enumerates after ~50 ms)
    Reset {
        /// Skip the interactive confirmation prompt
        #[arg(short = 'y', long)]
        yes: bool,
    },

    /// Reboot the device into BOOTSEL mode (USB Mass Storage for FW update)
    Bootsel {
        /// Skip the interactive confirmation prompt
        #[arg(short = 'y', long)]
        yes: bool,
    },

    /// Interactive REPL mode
    Interactive,

    /// Output operations
    Outputs {
        #[command(subcommand)]
        action: OutputsAction,
    },

    /// Safety settings: show, apply a safety.toml, keep the watchdog fed
    Safety {
        #[command(subcommand)]
        action: SafetyAction,
    },

    /// Set the watchdog timeout (100-10000 ms, 0 or 'off' disables it)
    #[command(long_about = WDOG_HELP)]
    Wdog {
        /// Timeout in milliseconds (100-10000), or 0 / 'off' to disable
        timeout: String,
    },

    /// Failsafe latch operations
    Failsafe {
        #[command(subcommand)]
        action: FailsafeAction,
    },

    /// Exclusive output groups (pins of a group are never ON together)
    Excl {
        #[command(subcommand)]
        action: ExclAction,
    },

    /// Failsafe when the USB link is lost (volatile, off after every restart)
    Usbsafe {
        /// 'on' trips the failsafe on USB loss, 'off' ignores it
        state: OnOff,
    },

    /// Show the cause of the last restart, the uptime and the boot id
    Rstinfo,
}

/// Help of `wdog`: the one-shot-process caveat must be visible.
const WDOG_HELP: &str = "Set the watchdog timeout (100-10000 ms; 0 or 'off' disables it).

Once enabled, the device trips when no command is answered for the timeout: \
every output goes to its default (def), the failsafe latch is set and stays \
set until 'tcapi failsafe clear'. Each tcapi invocation connects, runs and \
exits, so a watchdog enabled here trips the device once this process has \
ended and the timeout has elapsed. Run 'tcapi safety hold' to keep it fed \
until Ctrl+C. The setting is volatile: it is off again after every restart.";

#[derive(Clone, Copy, ValueEnum)]
enum OnOff {
    On,
    Off,
}

#[derive(Subcommand)]
enum OutputsAction {
    /// Return every output to its default (def) and stop a running pattern;
    /// works with the failsafe latch set and does not release it
    Reset,
}

#[derive(Subcommand)]
enum SafetyAction {
    /// Show the watchdog, failsafe latch, usbsafe and exclusive group count
    Status,
    /// Apply a safety.toml (excl, usbsafe, then wdog) and verify it
    #[command(long_about = APPLY_HELP)]
    Apply {
        /// Path to the safety.toml file
        file: String,
    },
    /// Keep the enabled watchdog fed until Ctrl+C (outputs are not changed)
    #[command(long_about = HOLD_HELP)]
    Hold,
}

const APPLY_HELP: &str = "Apply a safety.toml and verify it.

The commands are sent in this order: 'excl set' (or 'excl clear' when the file \
has no groups), 'usbsafe on|off', 'wdog <ms>'. The first error stops the apply; \
settings accepted before it stay in effect. The pins of the groups must \
already be outputs (apply the map first). If the file enables the watchdog, \
the device trips when this process has ended and the timeout has elapsed; \
run 'tcapi safety hold' to keep it fed.";

const HOLD_HELP: &str = "Keep the enabled watchdog fed until Ctrl+C.

Sends a heartbeat (ping every quarter of the timeout, at most 1 s) and checks \
the failsafe latch once a second. Ctrl+C ends the command without changing \
any output; the watchdog then trips after its timeout, as it does for any \
host that goes away. Fails if the watchdog is disabled (exit code 3) or the \
latch is already set (exit code 4), and exits with code 4 when the latch is \
set while holding.";

#[derive(Subcommand)]
enum FailsafeAction {
    /// Release the failsafe latch (outputs stay at their defaults)
    Clear,
}

#[derive(Subcommand)]
enum ExclAction {
    /// Replace all groups; each mask names 2 or more output pins (max 16 groups)
    Set {
        /// Group masks as hex or decimal, e.g. 0x0c 0x30
        #[arg(required = true, value_name = "MASK")]
        masks: Vec<String>,
    },
    /// Remove all groups
    Clear,
    /// List the groups
    Get,
}

#[derive(Subcommand)]
enum MapAction {
    /// Show current pin map
    Show,
    /// Set the pin map from KEY=VALUE pairs (all eight keys are required)
    Set {
        /// flags, dir, pu, pd, def, opol, ipol, edge — masks as hex or decimal
        /// (e.g. flags=0x00 dir=0x0f pu=0 pd=0x0c7fff00 def=0 opol=0 ipol=0 edge=0x0c7ffff0)
        #[arg(required = true, value_name = "KEY=VALUE")]
        args: Vec<String>,
    },
    /// Load pin map from TOML file
    Load {
        /// Path to TOML map file
        file: String,
    },
    /// Save current map to flash
    Save,
    /// Load default map (all 25 channels input, pull-down)
    Default,
}

#[derive(Subcommand)]
enum PatternAction {
    /// Append steps to the pattern buffer
    Add {
        /// Steps as <out_bits>:<hold_ticks> (1 tick = 200 ns), e.g. 0x1:5 0x0:5
        #[arg(required = true, value_name = "STEP")]
        steps: Vec<String>,
    },
    /// Upload a TOML pattern file, replacing the pattern buffer
    #[command(alias = "load")]
    Upload {
        /// Path to TOML pattern file
        file: String,
    },
    /// Run loaded pattern
    Run {
        /// Enable loop mode
        #[arg(long)]
        r#loop: bool,
        /// Repeat count (loop mode)
        #[arg(long, default_value = "1")]
        repeat: u16,
    },
    /// Stop running pattern
    Stop,
    /// Get pattern status
    Status,
    /// Reset the pattern engine (stops SM/DMA, clears buffer, state=Idle)
    Reset,
}

#[derive(Subcommand)]
enum TsAction {
    /// Arm timestamp capture
    Arm {
        /// Edge mask (hex, e.g. 0x3)
        mask: String,
        /// Edge mode: rising, falling, or both
        mode: String,
    },
    /// Show capture state and the number of pending events
    Status,
    /// Get captured timestamps
    Get {
        /// Maximum events to retrieve (1-85 per call)
        #[arg(default_value_t = TS_GET_MAX)]
        max_events: u16,
        /// Reconcile µs-boundary straddles across channels (groups events
        /// by count proximity and corrects DMA latch inconsistencies)
        #[arg(long)]
        reconcile: bool,
    },
    /// Disarm timestamp capture
    Disarm,
    /// One-shot capture (arm → wait → get → disarm)
    Capture {
        /// Edge mask (hex)
        mask: String,
        /// Edge mode: rising, falling, or both
        mode: String,
        /// Maximum events
        #[arg(default_value = "256")]
        max_events: u16,
    },
}

#[derive(Subcommand)]
enum SyncAction {
    /// Arm as sync slave
    Arm,
    /// Set as sync master
    Master,
    /// Start sync (master only)
    Start,
    /// Stop sync clock (master only)
    Stop,
    /// Get sync clock count
    Count,
    /// Disarm sync
    Disarm,
    /// Run full sync session (auto: arm slaves, master, start, count, stop)
    Run,
    /// Multi-device synchronized timestamp capture with reconcile
    Capture {
        /// Edge mask (hex, e.g. 0x3)
        mask: String,
        /// Edge mode: rising, falling, or both
        mode: String,
        /// Maximum events per device
        #[arg(long, default_value = "256")]
        events: u16,
        /// Capture duration in seconds
        #[arg(long, default_value = "5")]
        duration: u64,
    },
}

fn selector(cli: &Cli) -> device::DeviceSelector {
    device::DeviceSelector {
        uid: cli.uid.clone(),
        device: cli.device,
        port: cli.port.clone(),
    }
}

fn main() {
    let cli = match Cli::try_parse() {
        Ok(cli) => cli,
        Err(e) => {
            // `--help` / `--version` are printed to stdout and succeed; a
            // usage error is an argument error (exit code 3), not clap's 2.
            let code = if e.use_stderr() {
                exit::EXIT_ARGUMENT
            } else {
                0
            };
            let _ = e.print();
            process::exit(code);
        }
    };

    if let Err(e) = run(&cli) {
        let err_msg = format!("{e:#}");
        if cli.json {
            // Reporting the error is all that is left to do; if even stdout
            // is unwritable the exit code below still tells the story.
            let _ = output::print_json(&exit::error_json(&e));
        } else {
            eprintln!("Error: {err_msg}");
        }
        process::exit(exit::exit_code(&e));
    }
}

fn run(cli: &Cli) -> anyhow::Result<()> {
    let sel = selector(cli);

    match &cli.command {
        Command::List => commands::list::run(cli.json),
        Command::Version => commands::info::run_version(&sel, cli.json),
        Command::Caps => commands::info::run_caps(&sel, cli.json),
        Command::Info => commands::info::run_info(&sel, cli.json),
        Command::Devnum { value } => commands::info::run_devnum(&sel, value, cli.json),
        Command::Map { action } => match action {
            MapAction::Show => commands::map::run_show(&sel, cli.json),
            MapAction::Set { args } => commands::map::run_set(&sel, args, cli.json),
            MapAction::Load { file } => commands::map::run_load(&sel, file, cli.json),
            MapAction::Save => commands::map::run_save(&sel, cli.json),
            MapAction::Default => commands::map::run_default(&sel, cli.json),
        },
        Command::Read => commands::io::run_read(&sel, cli.json),
        Command::Write { values, mask } => commands::io::run_write(&sel, values, mask, cli.json),
        Command::State => commands::io::run_state(&sel, cli.json),
        Command::Watch => commands::io::run_watch(&sel, cli.json),
        Command::Pattern { action } => match action {
            PatternAction::Add { steps } => commands::pattern::run_add(&sel, steps, cli.json),
            PatternAction::Upload { file } => {
                commands::pattern::run_load_file(&sel, file, cli.json)
            }
            PatternAction::Run {
                r#loop: loop_mode,
                repeat,
            } => commands::pattern::run_run(&sel, *loop_mode, *repeat, cli.json),
            PatternAction::Stop => commands::pattern::run_stop(&sel, cli.json),
            PatternAction::Status => commands::pattern::run_status(&sel, cli.json),
            PatternAction::Reset => commands::pattern::run_reset(&sel, cli.json),
        },
        Command::Ts { action } => match action {
            TsAction::Arm { mask, mode } => {
                commands::timestamp::run_arm(&sel, mask, mode, cli.json)
            }
            TsAction::Status => commands::timestamp::run_status(&sel, cli.json),
            TsAction::Get {
                max_events,
                reconcile,
            } => commands::timestamp::run_get(&sel, *max_events, *reconcile, cli.json),
            TsAction::Disarm => commands::timestamp::run_disarm(&sel, cli.json),
            TsAction::Capture {
                mask,
                mode,
                max_events,
            } => commands::timestamp::run_capture(&sel, mask, mode, *max_events, cli.json),
        },
        Command::Sync { action } => match action {
            SyncAction::Arm => commands::sync::run_arm(&sel, cli.json),
            SyncAction::Master => commands::sync::run_master(&sel, cli.json),
            SyncAction::Start => commands::sync::run_start(&sel, cli.json),
            SyncAction::Stop => commands::sync::run_stop(&sel, cli.json),
            SyncAction::Count => commands::sync::run_count(&sel, cli.json),
            SyncAction::Disarm => commands::sync::run_disarm(&sel, cli.json),
            SyncAction::Run => commands::sync::run_sync_session(&sel, cli.json),
            SyncAction::Capture {
                mask,
                mode,
                events,
                duration,
            } => commands::sync::run_sync_capture(&sel, mask, mode, *events, *duration, cli.json),
        },
        Command::Raw { text } => commands::raw::run_raw(&sel, text, cli.json),
        Command::Reset { yes } => commands::reset::run_system_reset(&sel, *yes, cli.json),
        Command::Bootsel { yes } => commands::reset::run_bootsel(&sel, *yes, cli.json),
        Command::Interactive => commands::raw::run_interactive(&sel),
        Command::Outputs { action } => match action {
            OutputsAction::Reset => commands::safety::run_outputs_reset(&sel, cli.json),
        },
        Command::Safety { action } => match action {
            SafetyAction::Status => commands::safety::run_status(&sel, cli.json),
            SafetyAction::Apply { file } => commands::safety::run_apply(&sel, file, cli.json),
            SafetyAction::Hold => commands::safety::run_hold(&sel, cli.json),
        },
        Command::Wdog { timeout } => {
            commands::safety::run_wdog(&sel, parse::parse_wdog(timeout)?, cli.json)
        }
        Command::Failsafe { action } => match action {
            FailsafeAction::Clear => commands::safety::run_failsafe_clear(&sel, cli.json),
        },
        Command::Excl { action } => match action {
            ExclAction::Set { masks } => commands::safety::run_excl_set(&sel, masks, cli.json),
            ExclAction::Clear => commands::safety::run_excl_clear(&sel, cli.json),
            ExclAction::Get => commands::safety::run_excl_get(&sel, cli.json),
        },
        Command::Usbsafe { state } => {
            commands::safety::run_usbsafe(&sel, matches!(state, OnOff::On), cli.json)
        }
        Command::Rstinfo => commands::safety::run_rstinfo(&sel, cli.json),
    }
}
