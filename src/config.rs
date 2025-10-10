use crate::request::MaxHeaders;

/// Configuration for HTTP server behavior
#[derive(Debug, Clone, Copy)]
pub struct HttpConfig {
    /// Maximum number of headers to accept per request
    pub max_headers: MaxHeaders,
}

impl Default for HttpConfig {
    fn default() -> Self {
        Self {
            max_headers: MaxHeaders::Default,
        }
    }
}

impl HttpConfig {
    /// Create a new HTTP configuration with default settings
    pub fn new() -> Self {
        Self::default()
    }
    
    /// Set the maximum number of headers
    pub fn with_max_headers(mut self, max_headers: MaxHeaders) -> Self {
        self.max_headers = max_headers;
        self
    }
}

