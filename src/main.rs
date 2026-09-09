mod cli;

use anyhow::{Context, Result};
use clap::Parser;
use cli::Args;
use gekitai_solver::{certificate, forcing, player_server};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use tracing::{info, warn};

use gekitai_solver::checkpoint::{load_checkpoint, save_checkpoint};
use gekitai_solver::position::Position;
use gekitai_solver::proof::{solve_start_position, ProofConfig};
use gekitai_solver::rules::Rules;
use gekitai_solver::search::{search_iterative_deepening, SearchConfig, SearchMeta};
use gekitai_solver::tt::TranspositionTable;

fn main() -> Result<()> {
    // Logging: use RUST_LOG=info (or debug) to control verbosity.
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();

    let args = Args::parse();
    if let Some(path) = args.serve_certificate {
        return player_server::serve(path, args.port, args.public_origin_file);
    }
    if let Some(path) = &args.verify_forcing_certificate {
        return certificate::verify_file(path);
    }

    // Rayon threadpool
    let threads = args.threads.unwrap_or_else(num_cpus::get);
    if args.forcing.is_none() {
        rayon::ThreadPoolBuilder::new()
            .num_threads(threads)
            .build_global()
            .context("Failed to build Rayon global thread pool")?;
    }

    info!("Threads: {threads}");
    info!("Checkpoint: {}", args.checkpoint.display());

    // Ctrl+C stop flag
    let stop = Arc::new(AtomicBool::new(false));
    {
        let stop = stop.clone();
        ctrlc::set_handler(move || {
            stop.store(true, Ordering::Relaxed);
        })
        .context("Failed to set Ctrl+C handler")?;
    }

    let mut rules = Rules {
        repetition: args.repetition.into(),
        push_directions: args.push_directions.into(),
        simultaneous_win: args.simultaneous_win.into(),
        ..Rules::default()
    };

    if let Some(objective) = args.forcing {
        return forcing::run(
            rules,
            forcing::Config {
                objective,
                side: args.forcing_side,
                table_mb: args.tt_mb,
                threads,
                max_depth: args.max_depth,
                dir: args.forcing_dir,
                resume: args.forcing_resume,
                checkpoint_seconds: args.proof_checkpoint_seconds,
                checkpoint_mb: args.forcing_checkpoint_mb,
            },
            stop,
        );
    }

    if args.proof {
        let proof_max_ram_bytes = args
            .proof_max_ram_gb
            .map(|gb| gb.saturating_mul(1024 * 1024 * 1024));
        let proof_ram_ceiling_bytes = args.proof_ram_ceiling_gb.saturating_mul(1024 * 1024 * 1024);
        let proof_system_reserve_bytes = args
            .proof_system_reserve_gb
            .saturating_mul(1024 * 1024 * 1024);
        let proof_config = ProofConfig {
            max_states: args.proof_max_states,
            requested_max_ram_bytes: proof_max_ram_bytes,
            automatic_ram_ceiling_bytes: proof_ram_ceiling_bytes,
            system_reserve_bytes: proof_system_reserve_bytes,
            disk_dir: args.proof_disk_dir,
            resume: args.proof_resume,
            fast_ram: args.proof_fast_ram,
            checkpoint_seconds: args.proof_checkpoint_seconds,
        };
        let res = solve_start_position(&rules, &proof_config, stop)?;
        info!(
            "perfect-play start position: {} (states={}, edges={})",
            res.outcome.as_str(),
            res.states,
            res.edges
        );
        return Ok(());
    }

    // Load checkpoint if requested and present.
    let (mut tt, mut meta) = if args.resume && args.checkpoint.exists() {
        info!("Loading checkpoint...");
        let (ck_rules, ck_meta, ck_tt) = load_checkpoint(&args.checkpoint, args.no_compress)
            .context("load_checkpoint failed")?;

        if ck_rules != rules {
            warn!("Checkpoint rules != current rules. Using checkpoint rules.");
            rules = ck_rules;
        }

        (ck_tt, ck_meta)
    } else {
        info!("Starting new search.");
        let approx_entries = TranspositionTable::estimate_entries_from_mb(args.tt_mb);
        let tt = TranspositionTable::new(approx_entries);
        let meta = SearchMeta::default();
        (tt, meta)
    };

    // Start from initial position (side-to-move normalized: "us" is the side to move).
    // If you ever want to resume from a non-start position, store it in checkpoint metadata.
    let start_pos = Position::start();

    let cfg = SearchConfig {
        rules,
        max_depth: args.max_depth,
        save_every_depth: args.save_every_depth,
        checkpoint_path: args.checkpoint.clone(),
        compress_checkpoint: !args.no_compress,
        stop_flag: stop.clone(),
        once: args.once,
    };

    // Run search loop (iterative deepening).
    let result = search_iterative_deepening(&start_pos, &mut tt, &mut meta, &cfg);

    // Always attempt a final save if stop was requested, or after normal completion.
    if cfg.stop_flag.load(Ordering::Relaxed) {
        warn!("Stop requested. Writing checkpoint before exit...");
    } else {
        info!("Search completed. Writing final checkpoint...");
    }

    save_checkpoint(
        &cfg.checkpoint_path,
        cfg.compress_checkpoint,
        &cfg.rules,
        meta,
        &tt,
    )
    .context("save_checkpoint failed")?;

    result
}
