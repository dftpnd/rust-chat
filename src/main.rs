use teloxide::prelude::*;
use teloxide::types::Message;
use std::sync::Arc;
use std::sync::Mutex;
use std::collections::HashMap;
use serde::{Deserialize, Serialize};
use dotenvy::dotenv;

// Вызов OpenAI Chat Completions для генерации загадки
#[derive(Serialize)]
struct ChatMessage {
    role: String,
    content: String,
}

#[derive(Serialize)]
struct ChatRequestBody {
    model: String,
    messages: Vec<ChatMessage>,
}

#[derive(Deserialize)]
struct ChatResponse {
    choices: Vec<ChatChoice>,
}

#[derive(Deserialize)]
struct ChatChoice {
    message: ChatChoiceMessage,
}

#[derive(Deserialize)]
struct ChatChoiceMessage {
    content: String,
}

#[derive(Deserialize, Debug)]
struct RiddleJson {
    question: String,
    answer: String,
}

#[derive(Deserialize, Debug)]
struct AnswerCheckJson {
    correct: bool,
    feedback: String,
}

async fn check_answer_llm(api_key: &str, riddle_text: &str, user_answer: &str) -> Result<(bool, String), Box<dyn std::error::Error + Send + Sync>> {
    let client = reqwest::Client::new();
    let url = "https://api.openai.com/v1/chat/completions";

    let user_content = format!(
        r#"{{"action": "check_answer", "riddle_text": "{}", "user_answer": "{}"}}"#,
        riddle_text, user_answer
    );

    let body = ChatRequestBody {
        model: "gpt-5".to_string(),
        messages: vec![
            ChatMessage {
                role: "system".to_string(),
                content: "Ты бот загадок. Проверяй ответы на загадки и возвращай JSON с полями correct и feedback.".to_string(),
            },
            ChatMessage {
                role: "user".to_string(),
                content: user_content,
            },
        ],
    };

    let resp = client
        .post(url)
        .bearer_auth(api_key)
        .header("Content-Type", "application/json")
        .json(&body)
        .send()
        .await?;

    if !resp.status().is_success() {
        return Err(format!("OpenAI API error 1: {}", resp.status()).into());
    }

    let data: ChatResponse = resp.json().await?;
    let content = data
        .choices
        .get(0)
        .map(|c| c.message.content.trim().to_string())
        .ok_or("Empty choices from OpenAI")?;

    // Пытаемся распарсить JSON
    let parsed: AnswerCheckJson = serde_json::from_str(&content)?;
    Ok((parsed.correct, parsed.feedback))
}

#[derive(Clone)]
struct AppState {
    subscribers: Arc<Mutex<Vec<ChatId>>>,
    answers: Arc<Mutex<HashMap<ChatId, String>>>,
    winner: Arc<Mutex<Option<String>>>,
    current_question: Arc<Mutex<Option<(String, String)>>>,
    api_key: Option<String>,
    bot: Bot,
    enable_llm: Arc<Mutex<bool>>,
    last_request: Arc<Mutex<std::time::Instant>>,
}

impl AppState {
    fn new(api_key: Option<String>, bot: Bot) -> Self {
        Self {
            subscribers: Arc::new(Mutex::new(Vec::new())),
            answers: Arc::new(Mutex::new(HashMap::new())),
            winner: Arc::new(Mutex::new(None)),
            current_question: Arc::new(Mutex::new(None)),
            api_key,
            bot,
            enable_llm: Arc::new(Mutex::new(true)), // LLM включен по умолчанию
            last_request: Arc::new(Mutex::new(std::time::Instant::now())),
        }
    }

    async fn wait_for_rate_limit(&self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        const MIN_INTERVAL: std::time::Duration = std::time::Duration::from_secs(3); // Минимальный интервал между запросами
        const MAX_RETRIES: u32 = 3; // Максимальное количество попыток

        let mut retries = 0;
        loop {
            let now = std::time::Instant::now();

            // Получаем момент последнего запроса и сразу отпускаем мьютекс
            let last_instant = {
                let last_req = self.last_request.lock().unwrap();
                *last_req
            };

            let elapsed = now.duration_since(last_instant);

            if elapsed >= MIN_INTERVAL {
                // Обновляем время последнего запроса
                let mut last_req = self.last_request.lock().unwrap();
                *last_req = now;
                return Ok(());
            }

            retries += 1;
            if retries >= MAX_RETRIES {
                return Err("Превышено количество попыток запроса к API".into());
            }

            // Ждем оставшееся время + небольшой запас
            let wait_time = MIN_INTERVAL - elapsed + std::time::Duration::from_millis(100);
            tokio::time::sleep(wait_time).await;
        }
    }

    fn toggle_llm(&self) -> bool {
        let mut llm = self.enable_llm.lock().unwrap();
        *llm = !*llm;
        *llm
    }
}

impl AppState {
    async fn generate_riddle(&self) -> Result<(String, String), Box<dyn std::error::Error + Send + Sync>> {
        // Проверяем, включен ли LLM
        let llm_enabled = {
            let llm = self.enable_llm.lock().unwrap();
            *llm
        };

        // Если LLM выключен, возвращаем ошибку
        if !llm_enabled {
            return Err("LLM generation is disabled".into());
        }

        // Проверяем, не сгенерирована ли уже загадка и собираем данные для уведомлений
        let (question_exists, subscribers_to_notify) = {
            let q = self.current_question.lock().unwrap();
            if let Some((question, _)) = q.as_ref() {
                let subs = self.subscribers.lock().unwrap();
                (true, (question.clone(), subs.clone()))
            } else {
                (false, (String::new(), Vec::new()))
            }
        };

        // Если загадка существует, отправляем уведомления и возвращаем ошибку
        if question_exists {
            let (question, subscribers) = subscribers_to_notify;
            for chat_id in &subscribers {
                let _ = self.bot.send_message(*chat_id, format!("Загадка уже сгенерирована: {}", question)).await;
            }
            return Err("Riddle already generated".into());
        }

        let client = reqwest::Client::new();
        let url = "https://api.openai.com/v1/chat/completions";

        if let Some(key) = &self.api_key {
            // Сообщения по требованиям пользователя
            let body = ChatRequestBody {
                model: "gpt-5".to_string(),
                messages: vec![
                    ChatMessage {
                        role: "system".to_string(),
                        content: "Ты бот загадок. Придумывай короткие загадки с одним ответом. Ответ возвращай в JSON с полями 'question' и 'answer'.".to_string(),
                    },
                    ChatMessage {
                        role: "user".to_string(),
                        content: "{\"action\": \"new_riddle\", \"category\": \"природа\", \"difficulty\": \"средняя\"}".to_string(),
                    },
                ],
            };

            // Ждем, если нужно, перед отправкой запроса
            self.wait_for_rate_limit().await?;
            
            let resp = client
                .post(url)
                .bearer_auth(key)
                .header("Content-Type", "application/json")
                .json(&body)
                .send()
                .await?;

            if !resp.status().is_success() {
                return Err(format!("OpenAI API error 2: {}", resp.status()).into());
            }

            let data: ChatResponse = resp.json().await?;
            let content = data
                .choices
                .get(0)
                .map(|c| c.message.content.trim().to_string())
                .ok_or("Empty choices from OpenAI")?;

            // Ожидаем JSON с полями question/answer
            let parsed: RiddleJson = serde_json::from_str(&content)
                .or_else(|_| {
                    // Фоллбек: попытаться вытащить через примитивный парсинг
                    // Формат: Question: ...\nAnswer: ...
                    let lower = content.to_lowercase();
                    if let (Some(qi), Some(ai)) = (lower.find("question"), lower.find("answer")) {
                        let q = content[qi..].splitn(2, '\n').next().unwrap_or("");
                        let a = content[ai..].splitn(2, '\n').next().unwrap_or("");
                        let q = q.split(':').nth(1).unwrap_or("").trim().to_string();
                        let a = a.split(':').nth(1).unwrap_or("").trim().to_string();
                        if !q.is_empty() && !a.is_empty() {
                            Ok(RiddleJson { question: q, answer: a })
                        } else {
                            Err(serde_json::from_str::<RiddleJson>("{}").unwrap_err())
                        }
                    } else {
                        Err(serde_json::from_str::<RiddleJson>("{}").unwrap_err())
                    }
                })?;

            Ok((parsed.question, parsed.answer))
        } else {
            Err("API key not set".into())
        }
    }
}

#[tokio::main]
async fn main() {
    dotenv().ok();

    // Получаем OPENAI_API_KEY как Option<String> (None если не установлено)
    let api_key = std::env::var("OPENAI_API_KEY").ok();
    let token = std::env::var("TOKEN").unwrap_or_else(|_| "Unknown".to_string());

    let bot = Bot::new(token);
    
    pretty_env_logger::init();
    log::info!("Запускаем бота...");
    
    // Список подписчиков (chat_id)
    let state = AppState::new(api_key, bot.clone());
    
    let handler = move |bot: Bot, msg: Message| {
        let state = state.clone();
        
        async move {
            if let Some(text) = msg.text() {
                if text.starts_with("/togglellm") {
                    let enabled = state.toggle_llm();
                    let status = if enabled { "включена" } else { "отключена" };
                    bot.send_message(msg.chat.id, format!("Генерация через LLM {}", status)).await?;
                } else if text.starts_with("/start") {
                    // Добавляем пользователя в список подписчиков
                    let chat_id = msg.chat.id;
                    {
                        let mut subs = state.subscribers.lock().unwrap();
                        if !subs.contains(&chat_id) {
                            subs.push(chat_id);
                            log::info!("Новый подписчик: {:?}", chat_id);
                        }
                    }
                    bot.send_message(chat_id, "привет пиши /quiz").await?;
                } else if text.starts_with("/quiz") {
                    // Не даём стартовать, если квиз уже идёт (есть вопрос и нет победителя)
                    let (quiz_running, current_q) = {
                        let q = state.current_question.lock().unwrap();
                        let w = state.winner.lock().unwrap();
                        let is_running = q.is_some() && w.is_none();
                        let current_question = q.as_ref().map(|(q, _)| q.clone());
                        (is_running, current_question)
                    };
                    if quiz_running {
                        // Вместо предупреждения — отправляем текущий вопрос пользователю (если он есть)

                        if let Some(q_text) = current_q {
                            bot.send_message(msg.chat.id, format!("Текущий вопрос: {}", q_text)).await?;
                        } else {
                            // На всякий случай — если вопрос по какой-то причине отсутствует, вернуть старое сообщение
                            bot.send_message(msg.chat.id, "Квиз уже запущен. Дождитесь завершения (будет объявлен победитель). ").await?;
                        }
                    } else {
                        // Попытка сгенерировать вопрос через LLM; при любой ошибке используем локальный запасной вопрос
                        let (question, correct_answer) = match state.generate_riddle().await {
                            Ok(pair) => pair,
                            Err(e) => {
                                log::error!("Failed to generate riddle: {:?}", e);
                                return Ok(());
                            }
                        };
                        
                        // Сохраняем текущий вопрос и ответ
                        {
                            let mut q = state.current_question.lock().unwrap();
                            *q = Some((question.clone(), correct_answer.clone()));
                        }
                        
                        // Задаём вопрос всем
                        let subs_copy = {
                            let subs = state.subscribers.lock().unwrap();
                            subs.clone()
                        };
                        
                        // Сбрасываем ответы и победителя
                        {
                            let mut ans = state.answers.lock().unwrap();
                            ans.clear();
                        }
                        {
                            let mut w = state.winner.lock().unwrap();
                            *w = None;
                        }
                        
                        for chat_id in &subs_copy {
                            if let Err(e) = bot.send_message(*chat_id, &format!("❓ {}", question)).await {
                                log::error!("Ошибка отправки пользователю {:?}: {:?}", chat_id, e);
                            }
                        }
                        bot.send_message(msg.chat.id, "Викторина началась! Вопрос отправлен всем.").await?;
                    }
                } else if text.starts_with("/broadcast") {
                    // Рассылка всем подписчикам
                    let subs_copy = {
                        let subs = state.subscribers.lock().unwrap();
                        subs.clone()
                    };
                    
                    let subscribers_count = subs_copy.len();
                    let message = text.trim_start_matches("/broadcast").trim();
                    let message = if message.is_empty() { "Общее сообщение" } else { message };
                    
                    for chat_id in &subs_copy {
                        if let Err(e) = bot.send_message(*chat_id, message).await {
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
                    let (already_winner, quiz_question, api_key) = {
                        let w = state.winner.lock().unwrap();
                        let q = state.current_question.lock().unwrap();
                        let has_winner = w.is_some();
                        let question_copy = q.clone();
                        let api_key = state.api_key.clone();
                        (has_winner, question_copy, api_key)
                    };
                    
                    // Проверяем ответ через OpenAI (если нужно)
                    let (is_correct, feedback) = if already_winner {
                        (false, "".to_string())
                    } else if let Some((question, correct_answer)) = quiz_question.as_ref() {
                        // Используем скопированный из состояния api_key (из avoid holding lock)
                        if let Some(key) = api_key {
                            match check_answer_llm(&key, question, user_answer).await {
                                Ok((is_correct, feedback)) => (is_correct, feedback),
                                Err(e) => {
                                    log::error!("Answer check failed: {:?}", e);
                                    // Фоллбек на простое сравнение
                                    let simple_correct = user_answer.eq_ignore_ascii_case(correct_answer);
                                    (simple_correct, "".to_string())
                                }
                            }
                        } else {
                            // Нет API ключа - простой фоллбек
                            let simple_correct = user_answer.eq_ignore_ascii_case(correct_answer);
                            (simple_correct, "".to_string())
                        }
                    } else {
                        (false, "".to_string())
                    };
                    
                    if !already_winner {
                        if is_correct {
                            // Найден победитель!
                            {
                                let mut w = state.winner.lock().unwrap();
                                *w = Some(username.clone());
                            }
                            log::info!("Победитель найден: {}", username);
                            
                            // Отправляем всем о победителе
                            let subs_copy = {
                                let subs = state.subscribers.lock().unwrap();
                                subs.clone()
                            };
                            
                            let message = format!("🎉 Победитель: {}! Быстрее всех дал правильный ответ!", username);
                            for cid in &subs_copy {
                                if let Err(e) = bot.send_message(*cid, &message).await {
                                    log::error!("Ошибка отправки пользователю {:?}: {:?}", cid, e);
                                }
                            }
                            
                            bot.send_message(chat_id, "✅ Поздравляем! Вы победитель!").await?;
                        } else {
                            // Сохраняем ответ
                            {
                                let mut ans = state.answers.lock().unwrap();
                                ans.insert(chat_id, user_answer.to_string());
                            }
                            let response_msg = if !feedback.is_empty() {
                                format!("❌ Неправильно. {}. Попробуй ещё!", feedback)
                            } else {
                                "❌ Неправильно. Попробуй ещё!".to_string()
                            };
                            bot.send_message(chat_id, &response_msg).await?;
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


