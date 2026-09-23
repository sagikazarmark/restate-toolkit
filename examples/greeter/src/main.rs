use restate_sdk::http_server::HttpServer;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let config = greeter::config();
    let listener = config.endpoint.listener;
    let endpoint = greeter::endpoint(config)?;

    println!("serving the Greeter endpoint on {listener}");
    HttpServer::new(endpoint).listen_and_serve(listener).await;
    Ok(())
}
