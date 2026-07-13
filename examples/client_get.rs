//! Basic GET request example.
//!
//! Demonstrates connecting to a server, sending a GET request, and reading
//! the response body. Uses path-only URIs (no scheme/host) which the client
//! uses directly in the request line.

fn main() {
    // Connect to the server
    let mut client =
        may_minihttp::client::HttpClient::connect("127.0.0.1:8080").expect("failed to connect");

    // Send a GET request — uri can be path-only or a full URI
    let mut response = client
        .get("/".parse().expect("invalid uri"))
        .expect("GET request failed");

    println!("Status: {}", response.status());
    println!("Version: {:?}", response.version());

    for (key, value) in response.headers() {
        println!("{}: {}", key, value.to_str().unwrap_or("(invalid utf-8)"));
    }

    // Read the body
    let mut body = String::new();
    std::io::Read::read_to_string(&mut response, &mut body).expect("read body failed");
    println!("\nBody:\n{}", body);
}
