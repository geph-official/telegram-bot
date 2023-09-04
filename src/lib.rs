use std::{future::Future, time::Duration};

use anyhow::Context;
use isahc::{AsyncReadResponseExt, HttpClient, Request};
use serde_json::{json, Value};
use smol::Task;
use smol_timeout::TimeoutExt;

/// A client of the Telegram bot API.
pub struct TelegramBot {
    _task: Task<()>,
}

enum MessageType {
    Private,
    Group,
}

pub struct Message {
    text: String,
    from: String,
    msg_type: MessageType,
    chat_id: i64,
    replying: bool,
}

pub struct Response {
    text: String,
    chat_id: i64,
    reply_to_message_id: i64,
}

impl TelegramBot {
    /// Creates a new TelegramBot.
    pub fn new<
        Fun: Fn(Value) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Vec<Response>> + Send + Sync + 'static,
    >(
        bot_token: &str,
        bot_username: &str,
        msg_handler: Fun,
    ) -> Self {
        Self {
            _task: smol::spawn(handle_telegram(
                bot_token.to_owned(),
                bot_username.to_owned(),
                msg_handler,
            )),
        }
    }
}

async fn handle_telegram<Fun: Fn(Value) -> Fut, Fut: Future<Output = Vec<Response>>>(
    bot_token: String,
    bot_username: String,
    msg_handler: Fun,
) {
    let client = isahc::HttpClientBuilder::new()
        .max_connections(4)
        .build()
        .unwrap();

    let mut counter = 0;
    loop {
        log::info!("getting updates at {counter}");
        let fallible = async {
            let updates = call_api(
                &client,
                &bot_token,
                "getUpdates",
                json!({"timeout": 120, "offset": counter + 1, "allowed_updates": []}),
            )
            .await
            .context("cannot call telegram for updates")?;
            let updates: Vec<Value> = serde_json::from_value(updates)?;
            for update in updates {
                // we only support text msgs atm
                counter = counter.max(update["update_id"].as_i64().unwrap_or_default());
                if !update["message"]["text"].is_null() {
                    let responses = msg_handler(update).await;
                    // send response to telegram
                    let json_resps: Vec<Value> =
                        responses.iter().map(|resp| resp_json(resp)).collect();

                    for r in json_resps {
                        call_api(&client, &bot_token, "sendMessage", r)
                            .await
                            .context("cannot send reply back to telegram")?;
                    }
                }
            }
            anyhow::Ok(())
        };
        match fallible.timeout(Duration::from_secs(300)).await {
            Some(x) => {
                if let Err(err) = x {
                    log::error!("error getting updates: {:?}", err)
                }
            }
            None => log::error!("timed out getting telegram updates!"),
        }
    }
}

// Calls a Telegram API.
async fn call_api(
    client: &HttpClient,
    token: &str,
    method: &str,
    args: Value,
) -> anyhow::Result<Value> {
    let raw_res: Value = client
        .send_async(
            Request::post(format!("https://api.telegram.org/bot{}/{method}", token))
                .header("Content-Type", "application/json")
                .body(serde_json::to_vec(&args)?)?,
        )
        .await?
        .json()
        .await?;
    if raw_res["ok"].as_bool().unwrap_or(false) {
        Ok(raw_res["result"].clone())
    } else {
        anyhow::bail!(
            "telegram failed with error code {}",
            raw_res["error_code"]
                .as_i64()
                .context("could not parse error code as integer")?
        )
    }
}

// puts message into correct json format for telegram bot api
fn resp_json(resp: &Response) -> Value {
    json!({
        "chat_id": resp.chat_id,
        "text": resp.text,
        "reply_to_message_id": resp.reply_to_message_id,
    })
}