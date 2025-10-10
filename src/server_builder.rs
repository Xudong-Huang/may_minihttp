use crate::config::HttpConfig;
use crate::http_server::HttpServiceFactory;
use crate::request::MaxHeaders;
use may::coroutine;
use std::io;
use std::net::ToSocketAddrs;

/// Builder for creating and configuring HTTP servers
///
/// # Examples
///
/// ```no_run
/// use may_minihttp::{HttpServer, HttpService, Request, Response, MaxHeaders};
/// use std::io;
///
/// #[derive(Clone)]
/// struct MyService;
///
/// impl HttpService for MyService {
///     fn call(&mut self, _req: Request, rsp: &mut Response) -> io::Result<()> {
///         rsp.body("Hello World!");
///         Ok(())
///     }
/// }
///
/// // Start server with custom MaxHeaders
/// let server = HttpServer::new(MyService)
///     .max_headers(MaxHeaders::Large)
///     .bind("127.0.0.1:8080")
///     .unwrap();
/// ```
pub struct HttpServer<F> {
    factory: F,
    config: HttpConfig,
}

impl<F: HttpServiceFactory> HttpServer<F> {
    /// Create a new HTTP server with the given service factory
    pub fn new(factory: F) -> Self {
        Self {
            factory,
            config: HttpConfig::default(),
        }
    }
    
    /// Set the maximum number of headers to accept
    pub fn max_headers(mut self, max_headers: MaxHeaders) -> Self {
        self.config.max_headers = max_headers;
        self
    }
    
    /// Set the full HTTP configuration
    pub fn config(mut self, config: HttpConfig) -> Self {
        self.config = config;
        self
    }
    
    /// Bind to the given address and start the server
    pub fn bind<L: ToSocketAddrs>(self, addr: L) -> io::Result<coroutine::JoinHandle<()>> {
        // For now, we'll just use the factory's start method
        // TODO: Pass config through to control header limits
        self.factory.start(addr)
    }
}

