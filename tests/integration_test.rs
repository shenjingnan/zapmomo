use clap::Parser;
/// 集成测试示例
use zapmomo::cli::{self, Cli};

#[test]
fn test_cli_greet_output() {
    // 验证 CLI 可以正确解析 greet 命令
    let cli = Cli::try_parse_from(["test", "greet", "--name", "World"]).unwrap();
    assert!(matches!(cli.command.unwrap(), cli::Commands::Greet { .. }));
}

#[test]
fn test_cli_config_output() {
    // 验证 CLI 可以正确解析 config 命令
    let cli = Cli::try_parse_from(["test", "config"]).unwrap();
    assert!(matches!(cli.command.unwrap(), cli::Commands::Config));
}

#[test]
fn test_cli_speaker_install_model_parsing() {
    // 验证 CLI 可以正确解析 speaker install-model 命令
    let cli = Cli::try_parse_from(["test", "speaker", "install-model"]).unwrap();
    assert!(matches!(
        cli.command.unwrap(),
        cli::Commands::Speaker { .. }
    ));
    let cli = Cli::try_parse_from([
        "test",
        "speaker",
        "install-model",
        "--force",
        "--model-dir",
        "/tmp/spk",
    ])
    .unwrap();
    assert!(matches!(
        cli.command.unwrap(),
        cli::Commands::Speaker { .. }
    ));
}

#[test]
fn test_cli_speaker_subcommands_parsing() {
    // enroll：多值 wav positional
    let cli = Cli::try_parse_from([
        "test",
        "speaker",
        "enroll",
        "owner",
        "a.wav",
        "b.wav",
        "./samples/owner/",
    ])
    .unwrap();
    let cli::Commands::Speaker { cmd } = cli.command.unwrap() else {
        panic!("应解析为 speaker enroll");
    };
    let cli::SpeakerCmd::Enroll { speaker_id, wavs } = cmd else {
        panic!("应解析为 speaker enroll");
    };
    assert_eq!(speaker_id, "owner");
    assert_eq!(wavs.len(), 3);
    // identify
    let cli = Cli::try_parse_from(["test", "speaker", "identify", "test.wav"]).unwrap();
    assert!(matches!(
        cli.command.unwrap(),
        cli::Commands::Speaker {
            cmd: cli::SpeakerCmd::Identify { .. }
        }
    ));
    // verify
    let cli = Cli::try_parse_from(["test", "speaker", "verify", "owner", "test.wav"]).unwrap();
    assert!(matches!(
        cli.command.unwrap(),
        cli::Commands::Speaker {
            cmd: cli::SpeakerCmd::Verify { .. }
        }
    ));
    // list / remove
    let cli = Cli::try_parse_from(["test", "speaker", "list"]).unwrap();
    assert!(matches!(
        cli.command.unwrap(),
        cli::Commands::Speaker {
            cmd: cli::SpeakerCmd::List
        }
    ));
    let cli = Cli::try_parse_from(["test", "speaker", "remove", "owner"]).unwrap();
    assert!(matches!(
        cli.command.unwrap(),
        cli::Commands::Speaker {
            cmd: cli::SpeakerCmd::Remove { .. }
        }
    ));
}

#[tokio::test]
async fn test_run_config_returns_ok() {
    let cli = Cli::try_parse_from(["test", "config"]).unwrap();
    let result = cli::run(cli).await;
    assert!(result.is_ok());
}

#[tokio::test]
async fn test_run_greet_returns_ok() {
    let cli = Cli::try_parse_from(["test", "greet", "--name", "Integration"]).unwrap();
    let result = cli::run(cli).await;
    assert!(result.is_ok());
}

#[test]
fn test_datetime_iso_format() {
    let now = zapmomo::datetime::iso_timestamp_now();
    assert!(
        now.contains('T'),
        "ISO 8601 timestamp should contain T separator"
    );
}

#[test]
fn test_logging_init() {
    // 初始化日志不应 panic
    zapmomo::logging::init_logging();
}
