mod app_config;
mod event_handler;

use anyhow::{Context as _, Result};
use app_config::AppConfig;
use event_handler::Handler;
use std::env;

use serenity::framework::standard::StandardFramework;
use serenity::prelude::*;

/// メイン処理
#[tokio::main]
async fn main() -> Result<()> {
    let basedir = std::env::var("APP_BASEDIR").unwrap_or("bot/".to_string());

    // ログを初期化
    log4rs::init_file(format!("{}/log4rs.yml", basedir), Default::default())
        .context("log4rsの設定ファイルの読み込みに失敗")?;

    // フレームワークを初期化
    let framework = StandardFramework::new().configure(|c| c.prefix("~"));

    // 設定ファイルを読み込む
    let app_config = AppConfig::load_config(&basedir).context("設定ファイルの読み込みに失敗")?;

    // イベント受信リスナーを構築
    let handler = Handler::new(app_config).context("イベント受信リスナーの構築に失敗")?;

    // 環境変数のトークンを使用してDiscord APIを初期化
    let token = env::var("DISCORD_TOKEN").context("トークンが指定されていません")?;
    let intents = GatewayIntents::non_privileged()
        | GatewayIntents::MESSAGE_CONTENT
        | GatewayIntents::GUILD_MEMBERS;
    let mut client = Client::builder(token, intents)
        .event_handler(handler)
        .framework(framework)
        .await
        .context("Botの初期化に失敗")?;

    // イベント受信を開始
    client
        .start()
        .await
        .context("Bot動作中にエラーが発生しました")?;

    Ok(())
}
