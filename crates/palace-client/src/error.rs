use std::fmt;

/// Everything the runtime can fail at.
#[derive(Debug)]
pub enum ClientError {
    /// A socket operation failed.
    Io(std::io::Error),
    /// A frame or message body did not decode.
    Wire(palace_wire::error::WireError),
    /// The asset pipeline rejected a transfer.
    Asset(palace_asset::AssetError),
    /// A room descriptor did not parse.
    Room(String),
    /// The compositor could not produce a frame.
    Render(palace_render::RenderError),
    /// The peer closed the connection.
    Disconnected,
    /// The server sent something we cannot act on.
    Protocol(String),
    /// The caller asked for something impossible.
    Config(String),
}

impl fmt::Display for ClientError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ClientError::Io(e) => write!(f, "io: {e}"),
            ClientError::Wire(e) => write!(f, "wire: {e}"),
            ClientError::Asset(e) => write!(f, "asset: {e}"),
            ClientError::Room(d) => write!(f, "room: {d}"),
            ClientError::Render(e) => write!(f, "render: {e}"),
            ClientError::Disconnected => write!(f, "server closed the connection"),
            ClientError::Protocol(d) => write!(f, "protocol: {d}"),
            ClientError::Config(d) => write!(f, "config: {d}"),
        }
    }
}

impl std::error::Error for ClientError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            ClientError::Io(e) => Some(e),
            ClientError::Wire(e) => Some(e),
            ClientError::Asset(e) => Some(e),
            ClientError::Render(e) => Some(e),
            _ => None,
        }
    }
}

impl From<std::io::Error> for ClientError {
    fn from(e: std::io::Error) -> Self {
        ClientError::Io(e)
    }
}

impl From<palace_wire::error::WireError> for ClientError {
    fn from(e: palace_wire::error::WireError) -> Self {
        ClientError::Wire(e)
    }
}

impl From<palace_asset::AssetError> for ClientError {
    fn from(e: palace_asset::AssetError) -> Self {
        ClientError::Asset(e)
    }
}

impl From<palace_render::RenderError> for ClientError {
    fn from(e: palace_render::RenderError) -> Self {
        ClientError::Render(e)
    }
}

/// The runtime's result type.
pub type Result<T> = std::result::Result<T, ClientError>;
