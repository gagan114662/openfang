use clap::Parser;
use openfang_experiments::config::ExperimentConfig;
use openfang_experiments::mock::{MockDriver, MockMutator, MockScorer};
use openfang_experiments::runner::ExperimentRunner;
use std::path::PathBuf;
use std::sync::Arc;
use tracing::info;

#[derive(Parser)]
#[command(name = "openfang-experiments", about = "Autoresearch experiment loop")]
struct Cli {
    #[arg(short, long, help = "Path to experiment config TOML")]
    config: PathBuf,
    #[arg(short, long, help = "Output directory for results")]
    output: Option<PathBuf>,
    #[arg(short, long, help = "Verbose logging")]
    verbose: bool,
    #[arg(long, help = "Dry run with mock executor (no API calls)")]
    dry_run: bool,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    let filter = if cli.verbose {
        "openfang_experiments=debug,openfang_runtime=info"
    } else {
        "openfang_experiments=info"
    };
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .init();

    let mut config = ExperimentConfig::load(&cli.config)?;
    if let Some(output) = cli.output {
        config.output_dir = Some(output);
    }

    let _sentry_guard = if let Some(ref sentry_spec) = config.sentry {
        sentry_spec.dsn.as_ref().map(|dsn| {
            sentry::init(sentry::ClientOptions {
                dsn: dsn.parse().ok(),
                environment: Some(sentry_spec.environment.clone().into()),
                traces_sample_rate: sentry_spec.traces_sample_rate,
                ..Default::default()
            })
        })
    } else {
        None
    };

    let tx = sentry::start_transaction(sentry::TransactionContext::new(
        &format!("experiment.{}", config.name),
        "experiment",
    ));
    tx.set_data("experiment_name", config.name.clone().into());
    tx.set_data("model", config.model.model.clone().into());
    tx.set_data("provider", config.model.provider.clone().into());
    tx.set_data(
        "scoring_strategy",
        config.scoring_strategy_name().into(),
    );
    tx.set_data(
        "mutation_strategy",
        config.mutation_strategy_name().into(),
    );
    tx.set_data(
        "max_iterations",
        sentry::protocol::Value::from(config.max_iterations as u64),
    );

    sentry::configure_scope(|scope| {
        scope.set_span(Some(tx.clone().into()));
    });

    let summary = if cli.dry_run {
        info!("dry-run mode: using mock driver, scorer, and mutator");
        let driver = Arc::new(MockDriver::new(vec![
            "I understand your concern. Let me help you with your order. I'm sorry for the delay and I'll look into the tracking information right away.".into(),
        ]));
        let scorer = Box::new(MockScorer::new(vec![50.0, 65.0, 75.0, 70.0, 80.0, 60.0, 85.0, 72.0, 90.0, 78.0]));
        let mutator = Box::new(MockMutator::new(vec![
            format!("{} Be empathetic.", config.base_prompt),
            format!("{} Always acknowledge the issue first.", config.base_prompt),
            format!("{} Reference order numbers when mentioned.", config.base_prompt),
        ]));
        let runner = ExperimentRunner::new_with_deps(config, driver, scorer, mutator)?;
        runner.run().await?
    } else {
        let runner = ExperimentRunner::new(config)?;
        runner.run().await?
    };

    tx.set_data("best_score", sentry::protocol::Value::from(summary.best_score));
    tx.set_data(
        "best_iteration",
        sentry::protocol::Value::from(summary.best_iteration as u64),
    );
    tx.set_status(sentry::protocol::SpanStatus::Ok);
    tx.finish();

    println!("\n=== Experiment Complete ===");
    println!("Total iterations: {}", summary.total_iterations);
    println!("Best score:       {:.1}", summary.best_score);
    println!("Best iteration:   {}", summary.best_iteration);
    println!("Best prompt hash: {}", summary.best_prompt_hash);
    if let Some(ref path) = summary.best_prompt_path {
        println!("Best prompt:      {}", path.display());
    }
    println!("Results:          {}", summary.results_path.display());
    println!("Total tokens:     {} in / {} out", summary.total_tokens_input, summary.total_tokens_output);
    println!("Total cost:       ${:.4}", summary.total_cost_usd);

    Ok(())
}
