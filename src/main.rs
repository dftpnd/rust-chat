use teloxide::prelude::*;
use teloxide::types::Message;
use std::sync::Arc;
use std::sync::Mutex;

#[tokio::main]
async fn main() {
    const TOKEN: &str = "8245910738:AAHjwVmJXJ1qh3c-B_4PHhbZnG49zDdbZkQ";
    let bot = Bot::new(TOKEN);
    
    pretty_env_logger::init();
    log::info!("Запускаем бота...");
    
    // Список подписчиков (chat_id)
    let subscribers: Arc<Mutex<Vec<ChatId>>> = Arc::new(Mutex::new(Vec::new()));
    
    let handler = move |bot: Bot, msg: Message| {
        let subscribers = subscribers.clone();
        
        async move {
            if let Some(text) = msg.text() {
                if text.starts_with("/start") {
                    // Добавляем пользователя в список подписчиков
                    let chat_id = msg.chat.id;
                    {
                        let mut subs = subscribers.lock().unwrap();
                        if !subs.contains(&chat_id) {
                            subs.push(chat_id);
                            log::info!("Новый подписчик: {:?}", chat_id);
                        }
                    }
                    bot.send_message(chat_id, "привет").await?;
                } else if text.starts_with("/broadcast") {
                    // Рассылка всем подписчикам
                    let subs_copy = {
                        let subs = subscribers.lock().unwrap();
                        subs.clone()
                    };
                    
                    let subscribers_count = subs_copy.len();
                    let message = text.trim_start_matches("/broadcast").trim();
                    let message = if message.is_empty() { "Общее сообщение" } else { message };
                    
                    for chat_id in subs_copy {
                        if let Err(e) = bot.send_message(chat_id, message).await {
                            log::error!("Ошибка отправки пользователю {:?}: {:?}", chat_id, e);
                        }
                    }
                    bot.send_message(msg.chat.id, format!("Сообщение отправлено {} подписчикам", subscribers_count)).await?;
                }
            }
            Ok(())
        }
    };
    
    teloxide::repl(bot, handler).await;
}


