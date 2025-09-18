use anyhow::Result;
use async_trait::async_trait;
use reqwest::Client;
use rocketman::{
    connection::JetstreamConnection, handler, ingestion::LexiconIngestor,
    options::JetstreamOptions, types::event::Event,
};
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

mod resolve;

#[tokio::main]
async fn main() {
    // Load environment variables from .env file
    dotenv::dotenv().ok();

    // init the builder
    let opts = JetstreamOptions::builder()
        // your EXACT nsids
        .wanted_collections(vec!["fm.teal.alpha.feed.play".to_string()])
        .build();
    // create the jetstream connector
    let jetstream = JetstreamConnection::new(opts);

    // create your ingestors
    let mut ingestors: HashMap<String, Box<dyn LexiconIngestor + Send + Sync>> = HashMap::new();
    ingestors.insert(
        // your EXACT nsid
        "fm.teal.alpha.feed.play".to_string(),
        Box::new(MyCoolIngestor),
    );

    // tracks the last message we've processed
    let cursor: Arc<Mutex<Option<u64>>> = Arc::new(Mutex::new(None));

    // get channels
    let msg_rx = jetstream.get_msg_rx();
    let reconnect_tx = jetstream.get_reconnect_tx();

    // spawn a task to process messages from the queue.
    // this is a simple implementation, you can use a more complex one based on needs.
    let c_cursor = cursor.clone();
    tokio::spawn(async move {
        while let Ok(message) = msg_rx.recv_async().await {
            if let Err(e) =
                handler::handle_message(message, &ingestors, reconnect_tx.clone(), c_cursor.clone())
                    .await
            {
                eprintln!("Error processing message: {}", e);
            };
        }
    });

    // connect to jetstream
    // retries internally, but may fail if there is an extreme error.
    if let Err(e) = jetstream.connect(cursor.clone()).await {
        eprintln!("Failed to connect to Jetstream: {}", e);
        std::process::exit(1);
    }
}

pub struct MyCoolIngestor;

/// A cool ingestor implementation. Will just print the message. Does not do verification.
#[async_trait]
impl LexiconIngestor for MyCoolIngestor {
    async fn ingest(&self, message: Event<Value>) -> Result<()> {
        // Only process Create operations, ignore Delete operations
        if let Some(commit) = &message.commit {
            if !matches!(commit.operation, rocketman::types::event::Operation::Create) {
                return Ok(());
            }
        } else {
            return Ok(());
        }

        let client = Client::new();
        let url = std::env::var("DISCORD_WEBHOOK_URL")
            .expect("DISCORD_WEBHOOK_URL environment variable must be set");
        
        // Get resolver app view URL from environment
        let resolver_app_view = std::env::var("RESOLVER_APP_VIEW")
            .unwrap_or_else(|_| "https://bsky.social".to_string());
        
        // Safely extract track name and artist from the record
        let track_info = message
            .commit
            .as_ref()
            .and_then(|commit| commit.record.as_ref())
            .and_then(|record| {
                let track_name = record.get("trackName")?.as_str()?;
                let artists = record.get("artists")?.as_array()?;
                let artist_name = artists.first()?.get("artistName")?.as_str()?;
                Some(format!("{} by {}", track_name, artist_name))
            })
            .unwrap_or_else(|| "unknown track".to_string());

        // Resolve the handle from the DID
        let handle = match resolve::resolve_identity(&message.did, &resolver_app_view).await {
            Ok(resolved) => resolved.identity,
            Err(e) => {
                eprintln!("Failed to resolve handle for DID {}: {}", message.did, e);
                // Fallback to showing the DID if resolution fails
                message.did.clone()
            }
        };

        let payload = json!({
            "content": format!("{} is listening to {}", handle, track_info)
        });
        let response = client.post(url).json(&payload).send().await?;

        println!("{:?}", response.status());
        println!("{:?}", message);
        // Process message for default lexicon.
        Ok(())
    }
}
