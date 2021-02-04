// #![deny(warnings)]
use std::collections::HashMap;
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};
use std::{any::Any, thread};
use termion::input::TermRead;
use uuid::Uuid;

use futures::{FutureExt, StreamExt};
use tokio::sync::{mpsc, RwLock};
use tokio_stream::wrappers::UnboundedReceiverStream;
use warp::ws::{Message, WebSocket};
use warp::Filter;

/// Our global unique user id counter.
//static NEXT_USER_ID: AtomicUsize = AtomicUsize::new(1);

/// Our state of currently connected users.
///
/// - Key is their id
/// - Value is a sender of `warp::ws::Message`
//type Users = Arc<RwLock<HashMap<usize, mpsc::UnboundedSender<Result<Message, warp::Error>>>>>;
type Users = Arc<RwLock<HashMap<Uuid, mpsc::UnboundedSender<Result<Message, warp::Error>>>>>;

#[tokio::main]
async fn main() {
    pretty_env_logger::init();
    println!("Polling ...");

    println!("Starting up ...");
    // Keep track of all connected users, key is usize, value
    // is a websocket sender.
    let users = Users::default();
    // Turn our "state" into a new Filter...
    let thread_users = users.clone();
    let users = warp::any().map(move || users.clone());

    thread::spawn(move || {
        let mut stdin = termion::async_stdin().keys();
        loop {
            let input = stdin.next();
            if let Some(Ok(_key)) = input {
                match thread_users.try_read() {
                    Ok(guard) => {
                        //println!("--> {:?}", guard);
                        //print_type_of(&guard);
                        reply(&guard);
                    }
                    Err(_) => {
                        println!("Fail");
                    }
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

fn print_type_of<T>(_: &T) {
    println!("{}", std::any::type_name::<T>())
}

async fn user_connected(ws: WebSocket, users: Users) {
    // Use a counter to assign a new unique ID for this user.
    //let my_id = NEXT_USER_ID.fetch_add(1, Ordering::Relaxed);
    let user_id = Uuid::new_v4(); //.simple();

    eprintln!("new user connection: {}", user_id);

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
                eprintln!("websocket error(uid={}): {}", user_id, e);
                break;
            }
        };
        user_message(user_id, msg, &users).await;
    }

    // user_ws_rx stream will keep processing as long as the user stays
    // connected. Once they disconnect, then...
    user_disconnected(user_id, &users2).await;
}

fn reply(users: &HashMap<Uuid, mpsc::UnboundedSender<Result<Message, warp::Error>>>) {
    for (&uid, tx) in users.iter() {
        println!(">> Checking user {:?} ...", uid);
    }
}

//async fn user_message(my_id: usize, msg: Message, users: &Users) {
async fn user_message(user_id: Uuid, msg: Message, users: &Users) {
    // Skip any non-Text messages...
    let msg = if let Ok(s) = msg.to_str() {
        s
    } else {
        return;
    };

    //let new_msg = format!("<User#{}>: {}", my_id, msg);
    let new_msg = format!("<User#{}>: {}", user_id, msg);

    // New message from this user, send it to everyone else (except same uid)...
    for (&uid, tx) in users.read().await.iter() {
        println!("- Checking user {:?} ...", uid);
        //if my_id == uid {
        if user_id == uid {
            println!("  - Sending to user {:?} ...", uid);
            if let Err(_disconnected) = tx.send(Ok(Message::text(new_msg.clone()))) {
                // The tx is disconnected, our `user_disconnected` code
                // should be happening in another task, nothing more to
                // do here.
            }
            break;
        }
    }
}

async fn user_disconnected(user_id: Uuid, users: &Users) {
    eprintln!("good bye user: {}", user_id);

    // Stream closed up, so remove from the user list
    users.write().await.remove(&user_id);
}
