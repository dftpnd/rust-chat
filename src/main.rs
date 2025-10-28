use teloxide::prelude::*;
use teloxide::types::Message;
use std::sync::Arc;
use std::sync::Mutex;
use std::collections::HashMap;

// Функция генерации случайного вопроса
fn generate_random_question() -> (String, String) {
    let questions = vec![
        ("Какая столица России?", "Москва"),
        ("Сколько будет 2+2?", "4"),
        ("Какой самый большой океан?", "Тихий"),
        ("Сколько планет в солнечной системе?", "8"),
        ("Какое самое быстрое животное?", "Гепард"),
        ("Что означает 'hello' по-русски?", "Привет"),
        ("Сколько дней в году?", "365"),
        ("Какая самая длинная река в мире?", "Нил"),
        ("Сколько будет 5*5?", "25"),
        ("Какая планета ближе всего к Солнцу?", "Меркурий"),
    ];
    
    // Простой генератор на основе текущего времени
    let index = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() % questions.len() as u64;
    
    let (q, a) = questions[index as usize];
    (q.to_string(), a.to_string())
}

#[tokio::main]
async fn main() {
    const TOKEN: &str = "8245910738:AAHjwVmJXJ1qh3c-B_4PHhbZnG49zDdbZkQ";
    let bot = Bot::new(TOKEN);
    
    pretty_env_logger::init();
    log::info!("Запускаем бота...");
    
    // Список подписчиков (chat_id)
    let subscribers: Arc<Mutex<Vec<ChatId>>> = Arc::new(Mutex::new(Vec::new()));
    
    // Хранилище ответов: chat_id -> ответ
    let answers: Arc<Mutex<HashMap<ChatId, String>>> = Arc::new(Mutex::new(HashMap::new()));
    let winner: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
    // Текущий вопрос и правильный ответ
    let current_question: Arc<Mutex<Option<(String, String)>>> = Arc::new(Mutex::new(None));
    
    let handler = move |bot: Bot, msg: Message| {
        let subscribers = subscribers.clone();
        let answers = answers.clone();
        let winner = winner.clone();
        let current_question = current_question.clone();
        
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
                    bot.send_message(chat_id, "привет пиши /quiz").await?;
                } else if text.starts_with("/quiz") {
                    // Генерируем новый вопрос
                    let (question, correct_answer) = generate_random_question();
                    
                    // Сохраняем текущий вопрос и ответ
                    {
                        let mut q = current_question.lock().unwrap();
                        *q = Some((question.clone(), correct_answer.clone()));
                    }
                    
                    // Задаём вопрос всем
                    let subs_copy = {
                        let subs = subscribers.lock().unwrap();
                        subs.clone()
                    };
                    
                    // Сбрасываем ответы
                    {
                        let mut ans = answers.lock().unwrap();
                        ans.clear();
                    }
                    {
                        let mut w = winner.lock().unwrap();
                        *w = None;
                    }
                    
                    for chat_id in subs_copy {
                        if let Err(e) = bot.send_message(chat_id, &format!("❓ {}", question)).await {
                            log::error!("Ошибка отправки пользователю {:?}: {:?}", chat_id, e);
                        }
                    }
                    bot.send_message(msg.chat.id, format!("Викторина началась! Вопрос отправлен всем.\nПравильный ответ: {}", correct_answer)).await?;
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
                } else {
                    // Пользователь отправил ответ на вопрос
                    let chat_id = msg.chat.id;
                    let user_answer = text.trim();
                    
                    let username = msg.from()
                        .and_then(|u| u.username.clone().or_else(|| Some(u.first_name.clone())))
                        .unwrap_or_else(|| format!("Unknown_{:?}", chat_id));
                    
                    // Проверяем, есть ли уже победитель и текущий вопрос
                    let (already_winner, is_correct) = {
                        let w = winner.lock().unwrap();
                        let q = current_question.lock().unwrap();
                        let has_winner = w.is_some();
                        let correct = if let Some((_, correct)) = q.as_ref() {
                            user_answer.eq_ignore_ascii_case(&correct.to_string())
                        } else {
                            false
                        };
                        (has_winner, correct)
                    };
                    
                    if !already_winner {
                        if is_correct {
                            // Найден победитель!
                            {
                                let mut w = winner.lock().unwrap();
                                *w = Some(username.clone());
                            }
                            log::info!("Победитель найден: {}", username);
                            
                            // Отправляем всем о победителе
                            let subs_copy = {
                                let subs = subscribers.lock().unwrap();
                                subs.clone()
                            };
                            
                            let message = format!("🎉 Победитель: {}! Быстрее всех дал правильный ответ!", username);
                            for cid in subs_copy {
                                if let Err(e) = bot.send_message(cid, &message).await {
                                    log::error!("Ошибка отправки пользователю {:?}: {:?}", cid, e);
                                }
                            }
                            
                            bot.send_message(chat_id, "✅ Поздравляем! Вы победитель!").await?;
                        } else {
                            // Сохраняем ответ
                            {
                                let mut ans = answers.lock().unwrap();
                                ans.insert(chat_id, user_answer.to_string());
                            }
                            bot.send_message(chat_id, "❌ Неправильно. Попробуй ещё!").await?;
                        }
                    } else {
                        bot.send_message(chat_id, "⏰ Победитель уже найден!").await?;
                    }
                }
            }
            Ok(())
        }
    };
    
    teloxide::repl(bot, handler).await;
}


