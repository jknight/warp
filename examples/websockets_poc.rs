// #![deny(warnings)]
use std::{collections::HashMap, usize};
use std::io::{self, BufRead};

use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};

use std::{thread};
use futures::{FutureExt, StreamExt};
use tokio::sync::{mpsc, RwLock};
use tokio_stream::wrappers::UnboundedReceiverStream;
use warp::ws::{Message, WebSocket};
use warp::Filter;

/// Our global unique user id counter.
static NEXT_USER_ID: AtomicUsize = AtomicUsize::new(1);
type Users = Arc<RwLock<HashMap<usize, mpsc::UnboundedSender<Result<Message, warp::Error>>>>>;

#[tokio::main]
async fn main() {
    pretty_env_logger::init();
    println!("
    -----------------
    Starting up a warp websocket on ws://127.0.0.1:8080/api ...
    Once it's running, connect to the websocket to be issued an id.
    (Weasel WebSocket Client brower plugin is a good option for testing connections)
    Once you're connected with one or more sessions, simulate replying on an event by
    entering a message into the console with their subscriber number followed by a message:

    1 Here is a message for subscriber #1
    
    or:

    2 Here is a message for subscriber number 2 hello 

    -----------------
    ");


    // Keep track of all connected users, key is usize, value
    // is a websocket sender.
    let users = Users::default();

    // Turn our "state" into a new Filter...
    let thread_users = users.clone();
    let users = warp::any().map(move || users.clone());

    // Listen for console input on a background thread
    thread::spawn(move || {
        let stdin = io::stdin();
        for line in stdin.lock().lines() {
            let input = line.unwrap();
            let (a, b) = input.split_at(1);
            let user_id = a.parse::<usize>().unwrap();
            let message = b.parse::<String>().unwrap();
            println!("You entered: {}", input);
            match thread_users.try_read() {
                    Ok(users) => {
                        // Simulate an event here - reply to subscribers ...
                        // in this mockup we take console input as the response to 
                        // simulate a callback / reply on the websocket
                        let message = format!("user_id: {} \n {}", user_id, message);
                        reply(user_id, &message, &users);
                    }
                    Err(_) => {
                        println!("Failed to parse read users");
                    }
                }
        }
    });
    // GET /api -> websocket upgrade
    let api = warp::path("api")
        // The `ws()` filter will prepare Websocket handshake...
        .and(warp::ws())
        .and(users)
        .map(|ws: warp::ws::Ws, users| {
            // This will call our function if the handshake succeeds.
            ws.on_upgrade(move |socket| user_connected(socket, users))
        });

    let routes = api; //index.or(api);

    warp::serve(routes).run(([127, 0, 0, 1], 8080)).await;
}

async fn user_connected(ws: WebSocket, users: Users) {
    // Use a counter to assign a new unique ID for this user.
    let user_id = NEXT_USER_ID.fetch_add(1, Ordering::Relaxed);
    //let user_id = Uuid::new_v4().hyphenated().to_string();

    println!("new user connection: {}", user_id);

    // Split the socket into a sender and receive of messages.
    let (user_ws_tx, mut user_ws_rx) = ws.split();

    // Use an unbounded channel to handle buffering and flushing of messages
    // to the websocket...
    let (tx, rx) = mpsc::unbounded_channel();
    let rx = UnboundedReceiverStream::new(rx);
    tokio::task::spawn(rx.forward(user_ws_tx).map(|result| {
        if let Err(e) = result {
            eprintln!("websocket send error: {}", e);
        }
    }));

    // Save the sender in our list of connected users.
    users.write().await.insert(user_id, tx);

    // Return a `Future` that is basically a state machine managing
    // this specific user's connection.

    // Make an extra clone to give to our disconnection handler...
    let users2 = users.clone();

    while let Some(result) = user_ws_rx.next().await {
        let msg = match result {
            Ok(msg) => msg,
            Err(e) => {
                println!("websocket error(uid={}): {}", user_id, e);
                break;
            }
        };
        user_message(&user_id, msg, &users).await;
    }

    // user_ws_rx stream will keep processing as long as the user stays
    // connected. Once they disconnect, then...
    user_disconnected(user_id, &users2).await;
}

fn reply(user_id: usize, message: &str, users: &HashMap<usize, mpsc::UnboundedSender<Result<Message, warp::Error>>>) {
    for (&uid, tx) in users.iter() {
        println!(">> Sending message back to user {:?} ...", uid);
        if user_id == uid {
            println!("  - Sending to user {:?} ...", uid);
            //let message = format!("hello I am responding to you user {}", uid);
            if let Err(_disconnected) = tx.send(Ok(Message::text(message))) {
                // The tx is disconnected, our `user_disconnected` code
                // should be happening in another task, nothing more to
                // do here.
            }
            break;
        }
        
    }
}

async fn user_message(user_id: &usize, msg: Message, users: &Users) {
    // Skip any non-Text messages...
    let msg = if let Ok(s) = msg.to_str() {
        s
    } else {
        return;
    };

    let new_msg = format!("Connection successful. Your user id is: {} / your message was: {} ", user_id, msg);

    // Reply to the user by id
    for (uid, tx) in users.read().await.iter() {
        println!("- Checking user {:?} ...", uid);
        if user_id == uid {
            println!("  - Sending to user {:?} ...", uid);
            if let Err(_disconnected) = tx.send(Ok(Message::text(new_msg.clone()))) {
                // The tx is disconnected, our `user_disconnected` code
                // should be happening in another task, nothing more to
                // do here.
            }
            break;  // we already set the target user a message - no need to keep checking
        }
    }
}

async fn user_disconnected(user_id: usize, users: &Users) {
    eprintln!("good bye user: {}", user_id);
    // Stream closed up, so remove from the user list
    users.write().await.remove(&user_id);
}
