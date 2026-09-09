use clap::{Parser, ValueEnum};
use gekitai_solver::{
    forcing,
    rules::{PushDirections, RepetitionRule, SimultaneousWin},
};
use std::path::PathBuf;

#[derive(Clone, Copy, Debug, ValueEnum)]
pub(crate) enum RepetitionMode {
    PathRepeatIsDraw,
    Forbidden,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
pub(crate) enum PushMode {
    AllEight,
    Orthogonal,
}

impl From<PushMode> for PushDirections {
    fn from(value: PushMode) -> Self {
        match value {
            PushMode::AllEight => PushDirections::AllEight,
            PushMode::Orthogonal => PushDirections::Orthogonal,
        }
    }
}

#[derive(Clone, Copy, Debug, ValueEnum)]
pub(crate) enum SimultaneousMode {
    Draw,
    MoverWins,
}

impl From<SimultaneousMode> for SimultaneousWin {
    fn from(value: SimultaneousMode) -> Self {
        match value {
            SimultaneousMode::Draw => SimultaneousWin::Draw,
            SimultaneousMode::MoverWins => SimultaneousWin::MoverWins,
        }
    }
}

impl From<RepetitionMode> for RepetitionRule {
    fn from(value: RepetitionMode) -> Self {
        match value {
            RepetitionMode::PathRepeatIsDraw => RepetitionRule::PathRepeatIsDraw,
            RepetitionMode::Forbidden => RepetitionRule::Forbidden,
        }
    }
}

/// Gekitai solver and strategy certificate checker.
/// See README for details.
#[derive(Parser, Debug)]
#[command(name = "gekitai-solver")]
#[command(version)]
#[command(about = "Gekitai search and independently verified strategies")]
pub(crate) struct Args {
    /// Serve a browser player; wait for the certificate and verify it before play.
    #[arg(long, conflicts_with_all = ["proof", "forcing", "verify_forcing_certificate"])]
    pub(crate) serve_certificate: Option<PathBuf>,
    /// Loopback browser port (player mode only).
    #[arg(long, default_value_t = 8765)]
    pub(crate) port: u16,
    /// File containing the exact public HTTPS origin; enables separate visitor games.
    #[arg(long, requires = "serve_certificate")]
    pub(crate) public_origin_file: Option<PathBuf>,
    /// Verify a portable forcing or avoidance strategy without running a search.
    #[arg(long, conflicts_with_all = ["proof", "forcing"])]
    pub(crate) verify_forcing_certificate: Option<PathBuf>,
    /// Certify a forced objective within a finite horizon (negative bounds remain unresolved).
    #[arg(long, value_enum, conflicts_with = "proof")]
    pub(crate) forcing: Option<forcing::Objective>,

    /// Which player tries to force the objective; the opponent tries to prevent it.
    #[arg(long, value_enum, default_value_t = forcing::Side::Both)]
    pub(crate) forcing_side: forcing::Side,

    /// Directory for separate, rule-bound forcing checkpoints and results.
    #[arg(long, default_value = "forcing-data")]
    pub(crate) forcing_dir: PathBuf,

    /// Resume forcing checkpoints; cache capacity may change.
    #[arg(long)]
    pub(crate) forcing_resume: bool,

    /// Maximum sparse checkpoint size per objective/player, in MiB.
    #[arg(long, default_value_t = 256)]
    pub(crate) forcing_checkpoint_mb: usize,

    /// Maximum depth (plies). Each ply is one placement.
    #[arg(long, default_value_t = 18)]
    pub(crate) max_depth: u16,

    /// Number of CPU threads for Rayon. Defaults to logical cores.
    #[arg(long)]
    pub(crate) threads: Option<usize>,

    /// Approx TT budget in MiB (rough heuristic; HashMap overhead varies).
    #[arg(long, default_value_t = 4096)]
    pub(crate) tt_mb: usize,

    /// Checkpoint file path (zstd-compressed by default).
    #[arg(long, default_value = "checkpoint.zst")]
    pub(crate) checkpoint: PathBuf,

    /// Resume from checkpoint if it exists.
    #[arg(long, default_value_t = false)]
    pub(crate) resume: bool,

    /// Save checkpoint after every completed depth iteration.
    #[arg(long, default_value_t = true)]
    pub(crate) save_every_depth: bool,

    /// Disable zstd compression for checkpoints (writes raw bincode stream).
    #[arg(long, default_value_t = false)]
    pub(crate) no_compress: bool,

    /// Do a single depth search (useful for debugging).
    #[arg(long, default_value_t = false)]
    pub(crate) once: bool,

    /// Repetition handling policy.
    #[arg(long, value_enum, default_value_t = RepetitionMode::PathRepeatIsDraw)]
    pub(crate) repetition: RepetitionMode,

    /// Which adjacent pieces are pushed. Official-style play uses all eight.
    #[arg(long, value_enum, default_value_t = PushMode::AllEight)]
    pub(crate) push_directions: PushMode,

    /// Result when the same move completes both owners' win conditions.
    #[arg(long, value_enum, default_value_t = SimultaneousMode::Draw)]
    pub(crate) simultaneous_win: SimultaneousMode,

    /// Run exact loopy proof solve from the start position (win/loss/draw).
    #[arg(long, default_value_t = false)]
    pub(crate) proof: bool,

    /// Optional safety limit for proof graph states.
    #[arg(long)]
    pub(crate) proof_max_states: Option<usize>,

    /// Optional RAM cap for proof mode in GiB (e.g. 100).
    #[arg(long)]
    pub(crate) proof_max_ram_gb: Option<u64>,

    /// Automatic proof RSS ceiling in GiB when --proof-max-ram-gb is omitted.
    #[arg(long, default_value_t = 192)]
    pub(crate) proof_ram_ceiling_gb: u64,

    /// RAM kept available for the desktop and other processes.
    #[arg(long, default_value_t = 32)]
    pub(crate) proof_system_reserve_gb: u64,

    /// Optional directory for proof-mode disk-backed edge files.
    #[arg(long)]
    pub(crate) proof_disk_dir: Option<PathBuf>,

    /// Resume proof mode from an interrupted checkpoint in --proof-disk-dir.
    #[arg(long, default_value_t = false)]
    pub(crate) proof_resume: bool,

    /// Maximum seconds between durable proof checkpoints.
    #[arg(long, default_value_t = 300)]
    pub(crate) proof_checkpoint_seconds: u64,

    /// Keep proof dedupe in RAM (faster, higher memory; reconstructed on resume).
    #[arg(long, default_value_t = false)]
    pub(crate) proof_fast_ram: bool,
}
