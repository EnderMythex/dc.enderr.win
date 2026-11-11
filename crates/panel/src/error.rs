use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
};

use crate::{discord::DiscordError, views};

#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error("not found")]
    NotFound,
    #[error("invalid CSRF token")]
    Csrf,
    #[error("bad request: {0}")]
    BadRequest(&'static str),
    #[error(transparent)]
    Discord(#[from] DiscordError),
    #[error("database: {0}")]
    Database(#[from] sqlx::Error),
    #[error("filesystem: {0}")]
    Io(#[from] std::io::Error),
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let (status, title, message) = match &self {
            Self::NotFound => (
                StatusCode::NOT_FOUND,
                "Page not found",
                "This page does not exist, or this bot is not assigned to your account.",
            ),
            Self::Csrf => (
                StatusCode::FORBIDDEN,
                "Session expired",
                "This form is no longer valid. Go back, reload the page and try again.",
            ),
            Self::BadRequest(message) => (StatusCode::BAD_REQUEST, "Bad request", *message),
            Self::Discord(DiscordError::Rejected(_)) => (
                StatusCode::UNPROCESSABLE_ENTITY,
                "Discord refused the change",
                "Discord did not accept this change. Go back and try something else.",
            ),
            Self::Discord(_) => (
                StatusCode::BAD_GATEWAY,
                "Discord is not responding",
                "Communication with Discord failed. Please try again in a moment.",
            ),
            Self::Database(_) | Self::Io(_) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                "Something went wrong",
                "An unexpected error occurred. Please try again in a moment.",
            ),
        };
        if status.is_server_error() {
            tracing::error!(error = %self, "request failed");
        }
        (status, views::error_page(status.as_u16(), title, message)).into_response()
    }
}
