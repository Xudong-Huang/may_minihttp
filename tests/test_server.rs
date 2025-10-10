//! Test server helpers with configurable MaxHeaders support

use may_minihttp::{HttpServer, HttpService, MaxHeaders, Request, Response};
use std::io;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

#[derive(Clone)]
pub struct TestService;

impl HttpService for TestService {
    fn call(&mut self, _req: Request, res: &mut Response) -> io::Result<()> {
        res.body("OK");
        Ok(())
    }
}

/// Test server with configurable MaxHeaders
pub struct ConfigurableTestServer {
    port: u16,
    _server_handle: thread::JoinHandle<()>,
    shutdown: Arc<AtomicBool>,
    max_headers: MaxHeaders,
}

impl ConfigurableTestServer {
    /// Create a test server with specific MaxHeaders configuration
    pub fn with_max_headers(port: u16, max_headers: MaxHeaders) -> Self {
        let shutdown = Arc::new(AtomicBool::new(false));
        let shutdown_clone = Arc::clone(&shutdown);

        // For now, start with default - we'll add configuration support next
        let server_handle = thread::spawn(move || {
            let _server = HttpServer(TestService)
                .start(&format!("127.0.0.1:{}", port))
                .expect("Failed to start test server");

            while !shutdown_clone.load(Ordering::Relaxed) {
                thread::sleep(Duration::from_millis(100));
            }
        });

        Self {
            port,
            _server_handle: server_handle,
            shutdown,
            max_headers,
        }
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    pub fn max_headers(&self) -> MaxHeaders {
        self.max_headers
    }
}

impl Drop for ConfigurableTestServer {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::Relaxed);
        // Give the server thread time to clean up
        thread::sleep(Duration::from_millis(200));
    }
}
