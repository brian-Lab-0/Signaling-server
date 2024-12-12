use tokio::sync::{broadcast, Mutex};
use warp::Filter;
use futures_util::{StreamExt, SinkExt};
use uuid::Uuid;
use std::collections::HashMap;
use std::sync::Arc;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Serialize, Deserialize, Debug, Clone)]
struct WebRtcMessage {
    msg_type: String,
    payload: Value,
    target: Option<String>,
    sender: Option<String>,
}

type Clients = Arc<Mutex<HashMap<String, broadcast::Sender<String>>>>;

#[tokio::main]
async fn main() {
    let clients: Clients = Arc::new(Mutex::new(HashMap::new()));

    let signaling_route = warp::path("signaling")
        .and(warp::ws())
        .and(with_clients(clients.clone()))
        .map(|ws: warp::ws::Ws, clients| {
            ws.on_upgrade(move |socket| handle_client(socket, clients))
        });

    println!("Signaling server running on ws://127.0.0.1:3030/signaling");
    warp::serve(signaling_route).run(([127, 0, 0, 1], 3030)).await;
}

fn with_clients(
    clients: Clients,
) -> impl Filter<Extract = (Clients,), Error = std::convert::Infallible> + Clone {
    warp::any().map(move || clients.clone())
}

async fn handle_client(ws: warp::ws::WebSocket, clients: Clients) {
    let (mut ws_tx, ws_rx) = ws.split();
    let client_id = Uuid::new_v4().to_string();
    println!("Client connected: {}", client_id);

    let (tx, mut rx) = broadcast::channel(100);
    clients.lock().await.insert(client_id.clone(), tx.clone());

    // Wrap `ws_rx` in `Arc<Mutex<_>>` for shared ownership
    let ws_rx = Arc::new(Mutex::new(ws_rx));

    // Handle incoming messages from the client
    let ws_rx_clone = Arc::clone(&ws_rx);
    let clients_clone = clients.clone();
    let client_id_clone = client_id.clone();
    tokio::spawn(async move {
        let mut ws_rx = ws_rx_clone.lock().await;
        while let Some(Ok(msg)) = ws_rx.next().await {
            if let Ok(text) = msg.to_str() {
                if let Ok(message) = serde_json::from_str::<WebRtcMessage>(text) {
                    handle_webrtc_message(&clients_clone, message, &client_id_clone).await;
                } else {
                    println!("Malformed message from client {}", client_id_clone);
                }
            }
        }
    });

    // Relay messages to the client
    tokio::spawn(async move {
        while let Ok(message) = rx.recv().await {
            if ws_tx.send(warp::ws::Message::text(message)).await.is_err() {
                break;
            }
        }
    });

    // Keep connection alive
    while ws_rx.lock().await.next().await.is_some() {
        // Do nothing
    }

    clients.lock().await.remove(&client_id);
    println!("Client disconnected: {}", client_id);
}

async fn handle_webrtc_message(
    clients: &Clients,
    message: WebRtcMessage,
    sender_id: &str,
) {
    let clients_lock = clients.lock().await;

    match message.msg_type.as_str() {
        "offer" | "answer" | "candidate" => {
            if let Some(ref target) = message.target {
                if let Some(tx) = clients_lock.get(target) {
                    let mut relay_message = message.clone();
                    relay_message.sender = Some(sender_id.to_string());
                    let _ = tx.send(serde_json::to_string(&relay_message).unwrap());
                } else {
                    println!("Target {} not connected", target);
                }
            } else {
                println!("Message missing target field");
            }
        }
        "discover" => {
            // Send back a list of available clients
            let peers: Vec<String> = clients_lock.keys().cloned().collect();
            if let Some(tx) = clients_lock.get(sender_id) {
                let response = WebRtcMessage {
                    msg_type: "peers".to_string(),
                    payload: serde_json::json!({ "peers": peers }),
                    target: None,
                    sender: None,
                };
                let _ = tx.send(serde_json::to_string(&response).unwrap());
            }
        }
        _ => {
            println!("Unknown message type: {}", message.msg_type);
        }
    }
}
