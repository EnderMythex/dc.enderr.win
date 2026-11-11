//! Talking to a running bot, and showing its answer after a redirect.

use std::{
    collections::HashMap,
    path::Path,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use dc_core::requests::{self, Answer, Request};

/// The bot polls its request folder every second; leave room for a slow
/// Discord call (an image upload, a few role changes).
const ANSWER_TIMEOUT: Duration = Duration::from_secs(10);
const POLL_INTERVAL: Duration = Duration::from_millis(150);

/// Sends a request to the bot and waits for its answer. A bot that is
/// stopped never answers: that becomes a failed answer saying so.
pub async fn ask_bot(bot_dir: &Path, request: &Request) -> Answer {
    let failed = |message: &str| Answer {
        ok: false,
        message: message.to_owned(),
    };
    let id = match requests::submit(bot_dir, request) {
        Ok(id) => id,
        Err(error) => {
            tracing::error!(%error, "writing a bot request failed");
            return failed("The request could not be sent to the bot.");
        }
    };
    let deadline = Instant::now() + ANSWER_TIMEOUT;
    while Instant::now() < deadline {
        tokio::time::sleep(POLL_INTERVAL).await;
        if let Some(answer) = requests::take_answer(bot_dir, &id) {
            return answer;
        }
    }
    requests::cancel(bot_dir, &id);
    failed("The bot did not answer. Check that it is running, then try again.")
}

/// One-time messages shown on the next page of a session. They carry text
/// written by the bot, so they are kept here rather than in the URL, where
/// anyone could forge them.
#[derive(Clone, Default)]
pub struct Notices(Arc<Mutex<HashMap<String, (Instant, Answer)>>>);

const NOTICE_LIFETIME: Duration = Duration::from_secs(120);

impl Notices {
    pub fn put(&self, session_key: &str, answer: Answer) {
        let mut notices = self.0.lock().unwrap();
        notices.retain(|_, (at, _)| at.elapsed() < NOTICE_LIFETIME);
        notices.insert(session_key.to_owned(), (Instant::now(), answer));
    }

    pub fn take(&self, session_key: &str) -> Option<Answer> {
        let (at, answer) = self.0.lock().unwrap().remove(session_key)?;
        (at.elapsed() < NOTICE_LIFETIME).then_some(answer)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn notices_are_shown_once() {
        let notices = Notices::default();
        let answer = Answer {
            ok: true,
            message: "Test sent to #general.".into(),
        };
        notices.put("session-a", answer.clone());
        assert_eq!(notices.take("session-b"), None);
        assert_eq!(notices.take("session-a"), Some(answer));
        assert_eq!(notices.take("session-a"), None);
    }
}
