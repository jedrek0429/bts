use std::{error::Error, fmt};

use async_trait::async_trait;
use futures_util::{SinkExt, StreamExt};
use tokio::net::TcpStream;
use tokio_tungstenite::{
    MaybeTlsStream, WebSocketStream, connect_async, tungstenite::Message as WebSocketMessage,
};

#[async_trait]
pub(crate) trait Connector: Send + Sync + 'static {
    async fn connect(&self, url: &str) -> Result<Box<dyn Connection>, TransportError>;
}

#[async_trait]
pub(crate) trait Connection: Send {
    async fn send_text(&mut self, message: String) -> Result<(), TransportError>;
    async fn receive_text(&mut self) -> Result<Option<String>, TransportError>;
    async fn close(&mut self) -> Result<(), TransportError>;
}

#[derive(Debug, Default)]
pub(crate) struct WebSocketConnector;

#[async_trait]
impl Connector for WebSocketConnector {
    async fn connect(&self, url: &str) -> Result<Box<dyn Connection>, TransportError> {
        let (stream, _) = connect_async(url)
            .await
            .map_err(|error| TransportError::new(error.to_string()))?;
        Ok(Box::new(WebSocketConnection { stream }))
    }
}

struct WebSocketConnection {
    stream: WebSocketStream<MaybeTlsStream<TcpStream>>,
}

#[async_trait]
impl Connection for WebSocketConnection {
    async fn send_text(&mut self, message: String) -> Result<(), TransportError> {
        self.stream
            .send(WebSocketMessage::Text(message.into()))
            .await
            .map_err(|error| TransportError::new(error.to_string()))
    }

    async fn receive_text(&mut self) -> Result<Option<String>, TransportError> {
        loop {
            let Some(message) = self.stream.next().await else {
                return Ok(None);
            };
            match message.map_err(|error| TransportError::new(error.to_string()))? {
                WebSocketMessage::Text(text) => return Ok(Some(text.to_string())),
                WebSocketMessage::Close(_) => return Ok(None),
                WebSocketMessage::Ping(payload) => {
                    self.stream
                        .send(WebSocketMessage::Pong(payload))
                        .await
                        .map_err(|error| TransportError::new(error.to_string()))?;
                }
                WebSocketMessage::Pong(_) => {}
                WebSocketMessage::Binary(_) => {
                    return Err(TransportError::new(
                        "Core sent a binary message on the terminal connection",
                    ));
                }
                WebSocketMessage::Frame(_) => {}
            }
        }
    }

    async fn close(&mut self) -> Result<(), TransportError> {
        self.stream
            .close(None)
            .await
            .map_err(|error| TransportError::new(error.to_string()))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TransportError(String);

impl TransportError {
    pub(crate) fn new(detail: impl Into<String>) -> Self {
        Self(detail.into())
    }
}

impl fmt::Display for TransportError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl Error for TransportError {}
