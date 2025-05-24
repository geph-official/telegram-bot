use std::{future::Future, time::Duration};

use anyhow::Context;
use isahc::{AsyncReadResponseExt, HttpClient, Request};
use serde_json::{json, Value};
use smol::Task;
use smol_timeout::TimeoutExt;

/// A client of the Telegram bot API.
pub struct TelegramBot {
    client: HttpClient,
    bot_token: String,
    _task: Task<()>,
}
pub struct Response {
    pub text: String,
    pub chat_id: i64,
    pub reply_to_message_id: Option<i64>,
}

impl TelegramBot {
    /// Creates a new TelegramBot.
    pub fn new<
        Fun: FnMut(Value) -> Fut + Send + 'static,
        Fut: Future<Output = anyhow::Result<Vec<Response>>> + Send + 'static,
    >(
        bot_token: &str,
        msg_handler: Fun,
    ) -> Self {
        let client = isahc::HttpClientBuilder::new()
            .max_connections(4)
            .build()
            .unwrap();
        Self {
            client: client.clone(),
            bot_token: bot_token.into(),
            _task: smolscale::spawn(handle_telegram(client, bot_token.to_owned(), msg_handler)),
        }
    }

    pub async fn send_msg(&self, to_send: Response) -> anyhow::Result<()> {
        call_api(
            &self.client,
            &self.bot_token,
            "sendMessage",
            resp_json(&to_send),
        )
        .await
        .context("cannot send reply back to telegram")?;
        Ok(())
    }

    pub async fn call_api(&self, method: &str, args: Value) -> anyhow::Result<Value> {
        call_api(&self.client, &self.bot_token, method, args).await
    }
}

async fn handle_telegram<
    Fun: FnMut(Value) -> Fut + Send,
    Fut: Future<Output = anyhow::Result<Vec<Response>>>,
>(
    client: HttpClient,
    bot_token: String,
    mut msg_handler: Fun,
) {
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
                    let responses = msg_handler(update).await?;
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
    if let Some(reply_to_msg_id) = resp.reply_to_message_id {
        json!({
            "chat_id": resp.chat_id,
            "text": resp.text,
            "reply_to_message_id": reply_to_msg_id,
        })
    } else {
        json!({
            "chat_id": resp.chat_id,
            "text": resp.text,
        })
    }
}
