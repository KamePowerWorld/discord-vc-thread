use std::{collections::HashMap, sync::Arc};

use anyhow::{Context as _, Result};
use log::{error, warn};
use serenity::model::{
    gateway::Ready,
    guild::Member,
    id::ChannelId,
    prelude::{Channel, ChannelType, GuildChannel},
    voice::VoiceState,
};

use crate::app_config::AppConfig;

use serenity::async_trait;
use serenity::prelude::*;

/// イベント受信リスナー
pub struct Handler {
    /// 設定
    app_config: AppConfig,
    /// VC→スレッドのマップ
    vc_to_thread: Arc<Mutex<HashMap<ChannelId, ChannelId>>>,
}

impl Handler {
    /// コンストラクタ
    pub fn new(app_config: AppConfig) -> Result<Self> {
        Ok(Self {
            app_config,
            vc_to_thread: Arc::new(Mutex::new(HashMap::new())),
        })
    }

    /// カスタムVCかどうか判定する
    fn is_custom_vc(&self, channel: &GuildChannel) -> bool {
        // チャンネルがVCでない場合は無視
        if channel.kind != ChannelType::Voice {
            return false;
        }

        // 親チャンネルID(≒カテゴリID)取得
        let parent_channel_id = match channel.parent_id {
            Some(id) => id,
            None => return false,
        };

        // 親チャンネルIDがカスタムVCカテゴリかどうか判定
        if parent_channel_id != self.app_config.discord.vc_category {
            return false;
        }

        // チャンネルが無視されるチャンネルかどうか判定
        if self
            .app_config
            .discord
            .vc_ignored_channels
            .contains(&channel.id)
        {
            return false;
        }

        true
    }

    /// 参加時にスレッドを作成する
    async fn create_or_mention_thread(
        &self,
        ctx: &Context,
        vc_channel_id: &ChannelId,
        member: &Member,
    ) -> Result<()> {
        // マップからスレッドのチャンネルIDを取得
        let map = self
            .vc_to_thread
            .lock()
            .await
            .get(vc_channel_id)
            .map(|c| c.clone());
        // 一度変数に入れてからmatchにいれないとロックされっぱなしになる
        match map {
            // スレッドが作成済みの場合
            Some(thread_id) => {
                // スレッドのメンバーを取得
                let members = thread_id
                    .get_thread_members(ctx)
                    .await
                    .context("スレッドメンバーの取得に失敗")?;
                // メンバーが存在しない場合
                if !members
                    .iter()
                    .filter_map(|m| m.user_id)
                    .any(|user_id| user_id == member.user.id)
                {
                    // 参加メッセージ
                    thread_id
                        .send_message(ctx, |m| {
                            m.content(format!("{} さんが参加しました。", member.mention()));
                            m
                        })
                        .await
                        .context("参加メッセージの送信に失敗")?;
                }
            }
            // スレッドが作成されていない場合
            None => {
                // チャンネル名を取得
                let channel_name = vc_channel_id
                    .name(&ctx)
                    .await
                    .unwrap_or("不明なチャンネル".to_string());
                // VCカテゴリチャンネルにメッセージを送信
                let thread_channel = self.app_config.discord.thread_channel;
                // メッセージを送信
                let message = thread_channel
                    .send_message(ctx, |m| {
                        m.content(format!(
                            "{} さんが新しいVCを作成しました。\nVCに参加する→ {}",
                            member.mention(),
                            vc_channel_id.mention(),
                        ));
                        m.allowed_mentions(|m| m.empty_users());
                        m
                    })
                    .await
                    .context("作成メッセージの送信に失敗")?;
                // スレッドを作成
                let thread = thread_channel
                    .create_public_thread(ctx, &message, |m| {
                        m.name(channel_name);
                        m.kind(ChannelType::PublicThread);
                        m
                    })
                    .await
                    .context("スレッドの作成に失敗")?;
                // VCのテキストにチャンネルメンションを追加
                vc_channel_id
                    .send_message(ctx, |m| {
                        m.content(format!("VCチャット→ {}", thread.mention()));
                        m
                    })
                    .await
                    .context("VCチャットの案内メッセージ作成に失敗")?;
                // 参加メッセージ
                thread
                    .send_message(ctx, |m| {
                        m.content(format!("{} さんがVCを作成しました。", member.mention(),));
                        m
                    })
                    .await
                    .context("参加メッセージの作成に失敗")?;

                // スレッドを登録
                self.vc_to_thread
                    .lock()
                    .await
                    .insert(vc_channel_id.clone(), thread.id);
            }
        };

        Ok(())
    }

    /// VC削除時にスレッドをアーカイブする
    async fn archive_thread(&self, ctx: &Context, vc_channel_id: &ChannelId) -> Result<()> {
        // マップからスレッドのチャンネルIDを取得
        let channel_id = self
            .vc_to_thread
            .lock()
            .await
            .get(vc_channel_id)
            .map(|c| c.clone());
        // 一度変数に入れてからmatchにいれないとロックされっぱなしになる
        match channel_id {
            // スレッドが作成済みの場合
            Some(thread_id) => {
                // スレッドをアーカイブ
                thread_id
                    .edit_thread(ctx, |t| {
                        t.archived(true);
                        t
                    })
                    .await
                    .context("スレッドのアーカイブに失敗")?;
            }
            // スレッドが作成されていない場合
            None => {}
        };

        Ok(())
    }

    /// VC名前変更時にスレッドをリネームする
    async fn rename_thread(&self, ctx: &Context, vc_channel_id: &ChannelId) -> Result<()> {
        // マップからスレッドのチャンネルIDを取得
        let channel_id = self
            .vc_to_thread
            .lock()
            .await
            .get(vc_channel_id)
            .map(|c| c.clone());
        // 一度変数に入れてからmatchにいれないとロックされっぱなしになる
        match channel_id {
            // スレッドが作成済みの場合
            Some(thread_id) => {
                // チャンネル名を取得
                let channel_name = vc_channel_id
                    .name(&ctx)
                    .await
                    .unwrap_or("不明なチャンネル".to_string());
                // スレッドをリネーム
                thread_id
                    .edit_thread(ctx, |t| {
                        t.name(channel_name);
                        t
                    })
                    .await
                    .context("スレッドのリネームに失敗")?;
            }
            // スレッドが作成されていない場合
            None => {}
        };

        Ok(())
    }
}

#[async_trait]
impl EventHandler for Handler {
    /// 準備完了時に呼ばれる
    async fn ready(&self, _ctx: Context, data_about_bot: Ready) {
        warn!("Bot準備完了: {}", data_about_bot.user.tag());
    }

    /// VC削除時
    async fn channel_delete(&self, ctx: Context, channel: &GuildChannel) {
        // カスタムVCでない場合は無視
        if !self.is_custom_vc(channel) {
            return;
        }

        // VCスレッドチャンネルをアーカイブ
        match self.archive_thread(&ctx, &channel.id).await {
            Ok(_) => {}
            Err(why) => {
                error!("VCスレッドチャンネルのアーカイブに失敗: {:?}", why);
                return;
            }
        }
    }

    /// VC名更新時
    async fn channel_update(&self, _ctx: Context, _old: Option<Channel>, new: Channel) {
        // チャンネルを取得
        let vc_channel = match new.guild() {
            Some(guild) => guild,
            None => return,
        };

        // カスタムVCでない場合は無視
        if !self.is_custom_vc(&vc_channel) {
            return;
        }

        // VCスレッドチャンネルをリネーム
        match self.rename_thread(&_ctx, &vc_channel.id).await {
            Ok(_) => {}
            Err(why) => {
                error!("VCスレッドチャンネルのリネームに失敗: {:?}", why);
                return;
            }
        }
    }

    /// VCに参加/退出した時
    async fn voice_state_update(&self, ctx: Context, _old: Option<VoiceState>, new: VoiceState) {
        // チャンネルID、ユーザーが存在しない場合は無視
        if let (Some(vc_channel_id), Some(member)) = (new.channel_id, new.member) {
            // チャンネルを取得
            let vc_channel = match vc_channel_id
                .to_channel(&ctx)
                .await
                .context("チャンネル取得失敗")
                .and_then(|c| c.guild().ok_or(anyhow::anyhow!("チャンネルが存在しません")))
            {
                Ok(channel) => channel,
                Err(why) => {
                    error!("チャンネルの取得に失敗: {:?}", why);
                    return;
                }
            };

            // カスタムVCでない場合は無視
            if !self.is_custom_vc(&vc_channel) {
                return;
            }

            // VCスレッドチャンネルを作成
            match self
                .create_or_mention_thread(&ctx, &vc_channel_id, &member)
                .await
            {
                Ok(_) => {}
                Err(why) => {
                    error!("VCスレッドチャンネルの作成/投稿に失敗: {:?}", why);
                    return;
                }
            }
        }
    }
}
