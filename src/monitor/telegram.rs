use std::sync::Arc;

use teloxide::prelude::*;
use teloxide::types::ChatId;
use teloxide::utils::command::BotCommands;
use tracing::{error, info, warn};

use crate::execution::ExecutionResult;
use crate::risk::RiskManager;
use crate::strategy::Opportunity;

/// Telegram bot: sends alerts and handles commands.
pub struct Notifier {
    bot: Bot,
    chat_id: ChatId,
}

#[derive(BotCommands, Clone)]
#[command(rename_rule = "lowercase")]
enum Command {
    /// Show current risk manager status
    Status,
    /// Trigger kill switch (halt all trading)
    Kill,
    /// Resume trading (reset kill switch)
    Resume,
    /// Show available commands
    Help,
}

impl Notifier {
    pub fn new(bot_token: &str, chat_id: i64) -> Self {
        let bot = Bot::new(bot_token);
        info!(chat_id = chat_id, "telegram notifier initialized");
        Self {
            bot,
            chat_id: ChatId(chat_id),
        }
    }

    /// Spawn a background task that listens for incoming commands.
    /// Only responds to messages from the authorized chat_id.
    pub fn start_command_handler(&self, risk_manager: Arc<RiskManager>) {
        let bot = self.bot.clone();
        let authorized_chat = self.chat_id;

        tokio::spawn(async move {
            let handler = Update::filter_message().endpoint(
                move |bot: Bot, msg: Message, risk: Arc<RiskManager>| {
                    let authorized = authorized_chat;
                    async move {
                        info!(
                            chat_id = msg.chat.id.0,
                            text = ?msg.text(),
                            "received telegram message"
                        );

                        // Only respond to authorized chat
                        if msg.chat.id != authorized {
                            warn!(
                                chat_id = msg.chat.id.0,
                                "ignoring command from unauthorized chat"
                            );
                            return respond(());
                        }

                        let text = match msg.text() {
                            Some(t) => t,
                            None => return respond(()),
                        };

                        match Command::parse(text, "") {
                            Ok(Command::Status) => {
                                let report = risk.status_report();
                                let reply = format!("📊 *Status*\n{report}");
                                bot.send_message(msg.chat.id, reply).await?;
                            }
                            Ok(Command::Kill) => {
                                risk.kill("manual kill via Telegram");
                                bot.send_message(
                                    msg.chat.id,
                                    "🛑 Kill switch activated. All trading halted.",
                                )
                                .await?;
                            }
                            Ok(Command::Resume) => {
                                risk.resume();
                                bot.send_message(
                                    msg.chat.id,
                                    "▶️ Trading resumed. Kill switch reset.",
                                )
                                .await?;
                            }
                            Ok(Command::Help) => {
                                let help = Command::descriptions().to_string();
                                bot.send_message(msg.chat.id, help).await?;
                            }
                            Err(_) => {
                                info!(text = text, "unrecognized command, ignoring");
                            }
                        }

                        respond(())
                    }
                },
            );

            info!("starting teloxide dispatcher (long polling)...");
            Dispatcher::builder(bot, handler)
                .dependencies(dptree::deps![risk_manager])
                .build()
                .dispatch()
                .await;
            warn!("teloxide dispatcher exited");
        });

        info!("telegram command handler started");
    }

    // --- Alert methods (one-way notifications) ---

    /// Send a raw text message.
    async fn send(&self, text: &str) {
        if let Err(e) = self.bot.send_message(self.chat_id, text).await {
            error!(error = %e, "failed to send telegram message");
        }
    }

    /// Notify: trade executed (success or failure).
    pub async fn notify_trade(&self, opportunity: &Opportunity, result: &ExecutionResult) {
        let icon = if result.success { "✅" } else { "⚠️" };
        let total_fee = opportunity.up_fee + opportunity.down_fee;
        let msg = format!(
            "{icon} *Trade {status}*\n\
             Asset: {asset}\n\
             Buy Up: {up_ask} (fee {up_fee})\n\
             Buy Down: {down_ask} (fee {down_fee})\n\
             Combined: {combined} → {after_fees} (fees {total_fee})\n\
             Profit/unit: {profit}\n\
             Units: {units} | Cost: {cost} USDC | P&L: {pnl} USDC\n\
             {message}",
            status = if result.success { "Filled" } else { "Failed" },
            asset = opportunity.asset,
            up_ask = opportunity.up_ask,
            up_fee = opportunity.up_fee,
            down_ask = opportunity.down_ask,
            down_fee = opportunity.down_fee,
            combined = opportunity.combined_ask,
            after_fees = opportunity.combined_ask_after_fees,
            total_fee = total_fee,
            profit = opportunity.profit_per_unit,
            units = opportunity.units,
            cost = opportunity.total_cost_usdc,
            pnl = opportunity.expected_profit_usdc,
            message = result.message,
        );
        self.send(&msg).await;
    }

    /// Notify: execution error (after retries exhausted).
    pub async fn notify_execution_error(&self, asset: &str, error: &str) {
        let msg = format!("❌ *Execution Error*\nAsset: {asset}\nError: {error}");
        self.send(&msg).await;
    }

    /// Notify: feed disconnected.
    pub async fn notify_disconnect(&self, source: &str) {
        let msg = format!("🔌 *Feed Disconnected*\nSource: {source}\nTrading halted.");
        self.send(&msg).await;
    }

    /// Notify: feed reconnected.
    pub async fn notify_reconnect(&self, source: &str) {
        let msg = format!("🔗 *Feed Reconnected*\nSource: {source}\nTrading may resume.");
        self.send(&msg).await;
    }

    /// Notify: kill switch triggered.
    pub async fn notify_kill_switch(&self, reason: &str) {
        let msg = format!("🛑 *Kill Switch Triggered*\nReason: {reason}\nAll trading halted.");
        self.send(&msg).await;
    }

    /// Notify: bot startup.
    pub async fn notify_startup(&self, mode: &str, market_count: usize) {
        let msg = format!("🚀 *MTB Started*\nMode: {mode}\nMarkets: {market_count}");
        self.send(&msg).await;
    }
}
