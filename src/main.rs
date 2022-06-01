use std::str::FromStr;
use std::{convert::Infallible, env, net::SocketAddr};
use std::error::Error;
use std;

use teloxide::{
    dispatching::{
        stop_token::AsyncStopToken,
        update_listeners::{self, StatefulListener},
    },
    prelude2::*,
    types::Update,
    utils::command::BotCommand
};
use tokio::sync::mpsc;
use warp::Filter;
use uuid::Uuid;

use reqwest::{StatusCode, Url};
use tokio_stream::wrappers::UnboundedReceiverStream;
use sqlx::{self, Row};

#[derive(BotCommand, Clone)]
#[command(rename = "lowercase", description = "These commands are supported:")]
enum Command {
    #[command(description = "display this text.")]
    Help,
    #[command(description = "Starting bot use")]
    Start,
    #[command(description = "Adding existing ID.", parse_with="split")]
    Add{ rattle_id: String},
    #[command(description = "Removing ID.", parse_with="split")]
    Remove{ rattle_id: String},
    #[command(description = "List of all your connected IDs.")]
    List,
    #[command(description = "Adding new ID.")]
    New,
    
}

async fn answer(
    bot: AutoSend<Bot>,
    message: Message,
    command: Command,
) -> Result<(), Box<dyn Error + Send + Sync>> {

    let uri = std::env::var("DATABASE_URL").unwrap();

    let pool = sqlx::PgPool::connect(uri.as_str()).await?;


    match command {
        Command::Help => bot.send_message(message.chat.id, Command::descriptions()).await?,
        Command::Start => {
            //Save Chat ID
            bot.send_message(message.chat.id, format!("Hi! Weclome to the RattleHead bot!\n{}", Command::descriptions())).await?
        },
        Command::Add{rattle_id} => {
            //Linking existing ID with Chat ID

            let new_uuid = Uuid::from_str(rattle_id.as_str());

            match new_uuid {
                Ok(uuid) => {


                    let result = sqlx::query("insert into rattle_telegram (external_id, chat_id) values ($1, $2);")
                    .bind::<sqlx::types::Uuid>(uuid)
                    .bind::<i64>(message.chat.id)
                    .fetch_optional(&pool)
                    .await;


                    match result {
                        Ok(_) => bot.send_message(message.chat.id, format!("ID: {} added.", rattle_id)).await?,
                        Err(error) => bot.send_message(message.chat.id, format!("Something goes wrong.\n {}", error.to_string())).await?

                    };
                },
                Err(_) =>
                {
                    bot.send_message(message.chat.id, format!("{} looks like not UUID.", rattle_id)).await?;
                }
            }


            bot.send_message(message.chat.id, format!("ID: {} added", rattle_id)).await?
        },
        Command::Remove{rattle_id} => {
            //Remove from DB
            let result = sqlx::query("delete from rattle_telegram where external_id = $1 and chat_id = $2;")
            .bind(&rattle_id)
            .bind(message.chat.id)
            .execute(&pool)
            .await;
            match result {
                Ok(_) =>             bot.send_message(message.chat.id, format!("ID: {} removed", rattle_id)).await?,
                Err(error) => bot.send_message(message.chat.id, format!("Something goes wrong.\n {}", error.to_string())).await?,
            }
        },
        Command::New => {
            //Generate new ID
            let new_rattle_id = Uuid::new_v4();
            
           let result = sqlx::query("insert into rattle_telegram (external_id, chat_id) values ($1, $2);")
           .bind::<sqlx::types::Uuid>(new_rattle_id)
           .bind::<i64>(message.chat.id)
           .execute(&pool)
           .await;

            match result {
                Ok(_) => bot.send_message(message.chat.id, format!("New ID: {}. Link", new_rattle_id)).await?,
                Err(error) => bot.send_message(message.chat.id, format!("Something goes wrong.\n {}", error.to_string())).await?,
            }

        },
        Command::List => {

            let result = sqlx::query("select external_id from rattle_telegram where chat_id = $1;")
            .bind::<i64>(message.chat.id)
            .fetch_all(&pool)
            .await;

             match result {
                 Ok(rows) => {

                    let mut rattle_id_list: Vec<String> = Vec::new();

                    for row in rows{
                        let id = row.get::<Uuid, _>("external_id");
                        rattle_id_list.push(id.to_string());
                    }

                     bot.send_message(message.chat.id, format!("ID List:\n {}", rattle_id_list.join("\n"))).await?
                    },
                 Err(error) => bot.send_message(message.chat.id, format!("Something goes wrong.\n {}", error.to_string())).await?,
             }


        }
    };
    Ok(())
}

#[tokio::main]
async fn main() {
    teloxide::enable_logging!();
    log::info!("Starting rattlehead_bot...");

    let bot = Bot::from_env().auto_send();

    teloxide::repls2::commands_repl_with_listener(bot.clone(), answer, webhook(bot).await, Command::ty())
    .await;
}

async fn handle_rejection(error: warp::Rejection) -> Result<impl warp::Reply, Infallible> {
    log::error!("Cannot process the request due to: {:?}", error);
    Ok(StatusCode::INTERNAL_SERVER_ERROR)
}

pub async fn webhook(bot: AutoSend<Bot>) -> impl update_listeners::UpdateListener<Infallible> {
    // Heroku auto defines a port value
    let teloxide_token = env::var("TELOXIDE_TOKEN").expect("TELOXIDE_TOKEN env variable missing");
    let port: u16 = env::var("PORT")
        .expect("PORT env variable missing")
        .parse()
        .expect("PORT value to be integer");
    // Heroku host example .: "heroku-ping-pong-bot.herokuapp.com"
    let host = env::var("HOST").expect("have HOST env variable");
    let path = format!("bot{}", teloxide_token);
    let url = Url::parse(&format!("https://{}/{}", host, path)).unwrap();

    bot.set_webhook(url).await.expect("Cannot setup a webhook");

    let (tx, rx) = mpsc::unbounded_channel();

    let server = warp::post()
        .and(warp::path(path))
        .and(warp::body::json())
        .map(move |update: Update| {
            tx.send(Ok(update)).expect("Cannot send an incoming update from the webhook");

            StatusCode::OK
        })
        .recover(handle_rejection);

    let (stop_token, stop_flag) = AsyncStopToken::new_pair();

    let addr = format!("0.0.0.0:{}", port).parse::<SocketAddr>().unwrap();
    let server = warp::serve(server);
    let (_addr, fut) = server.bind_with_graceful_shutdown(addr, stop_flag);

    // You might want to use serve.key_path/serve.cert_path methods here to
    // setup a self-signed TLS certificate.

    tokio::spawn(fut);
    let stream = UnboundedReceiverStream::new(rx);

    fn streamf<S, T>(state: &mut (S, T)) -> &mut S {
        &mut state.0
    }

    StatefulListener::new((stream, stop_token), streamf, |state: &mut (_, AsyncStopToken)| {
        state.1.clone()
    })
}
