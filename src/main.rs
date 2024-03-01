use std::collections::{HashMap, HashSet};

use askama::Template;
use bson::oid::ObjectId;
use chrono::Utc;
use mongodb::{bson::doc, options::ClientOptions, Client, Collection};
use serde::{Deserialize, Serialize};
use tokio_stream::StreamExt;
use warp::{reject::Rejection, reply::Reply, Filter};

mod secrets;

// Bluu Next font is kinda nice
// cargo watch -x run

#[tokio::main]
async fn main() -> mongodb::error::Result<()> {
    let client_options =
    ClientOptions::parse(secrets::mongo_url).await?;
    let client = Client::with_options(client_options)?;
    let database = client.database("test");
    let users = database.collection::<User>("users");
    let lists = database.collection::<List>("lists");
    let list_items = database.collection::<ListItem>("listitems");
    let friendships = database.collection::<Friendship>("friendships");

    database.run_command(doc! {"ping": 1}, None).await?;
    println!("Pinged your deployment. You successfully connected to MongoDB!");

    let lists = warp::path!("lists")
        .and(warp::any().map(move || lists.clone()))
        .and(warp::any().map(move || friendships.clone()))
        .and(warp::any().map(move || users.clone()))
        .and_then(lists_handler);

    warp::serve(lists)
        .run(([0, 0, 0, 0, 0, 0, 0, 0], 3030))
        .await;

    Ok(())
}

type WarpResult<T> = std::result::Result<T, Rejection>;

async fn lists_handler(
    lists: Collection<List>,
    friendships: Collection<Friendship>,
    users: Collection<User>,
) -> WarpResult<impl Reply> {
    let my_lists = lists
        .find(
            doc! {"userId": ObjectId::parse_str("5e2deeb7b337533136035340").unwrap()},
            None,
        )
        .await
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .await
        .unwrap();

    let friend_ids = friendships
        .find(
            doc! {"userId": ObjectId::parse_str("5e2deeb7b337533136035340").unwrap()},
            None,
        )
        .await
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .await
        .unwrap()
        .into_iter()
        .map(|x| x.friend_id)
        .collect::<Vec<_>>();

    // ^^ those two can probably be paralellized

    let friends_lists = lists
        .find(doc! { "userId": { "$in": friend_ids.clone() }}, None)
        .await
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .await
        .unwrap()
        .into_iter()
        .map(|x| (x.user_id.unwrap(), x))
        .collect::<Vec<_>>();

    let mut friends_to_lists = HashMap::new();

    // https://stackoverflow.com/a/30441736
    for (k, v) in friends_lists {
        friends_to_lists.entry(k).or_insert_with(Vec::new).push(v)
    }

    let friends = users
        .find(doc! {"_id": { "$in": friend_ids }}, None)
        .await
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .await
        .unwrap();

    // ^^ those two can probably be paralellized

    let friends_lists = friends
        .into_iter()
        .map(|friend| {
            let friend_lists = friends_to_lists
                .remove(&friend.id.unwrap())
                .unwrap_or(Vec::new());
            (friend, friend_lists)
        })
        .collect::<Vec<_>>();

    Ok(Lists {
        my_lists,
        friends_lists,
    })
}

#[derive(Template)]
#[template(path = "lists.html")]
struct Lists {
    my_lists: Vec<List>,
    friends_lists: Vec<(User, Vec<List>)>,
}

#[derive(Serialize, Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
struct User {
    #[serde(rename = "_id", skip_serializing_if = "Option::is_none")]
    id: Option<ObjectId>,
    first_name: String,
    last_name: String,
    #[serde(default)]
    access_level: AccessLevel,
    password: String,
    favorite_users: Vec<ObjectId>,
    #[serde(default)]
    theme: Theme,
    avatar_key: Option<String>,

    #[serde(with = "bson::serde_helpers::chrono_datetime_as_bson_datetime")]
    created_at: chrono::DateTime<Utc>,
    #[serde(with = "bson::serde_helpers::chrono_datetime_as_bson_datetime")]
    updated_at: chrono::DateTime<Utc>,
}

#[derive(Serialize, Deserialize, Debug, Default)]
enum AccessLevel {
    #[default]
    User,
    Admin,
}

#[derive(Serialize, Deserialize, Debug, Default)]
#[serde(rename_all = "snake_case")]
enum Theme {
    #[default]
    Light,
    Dark,
}

#[derive(Serialize, Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
struct List {
    #[serde(rename = "_id", skip_serializing_if = "Option::is_none")]
    id: Option<ObjectId>,
    user_id: Option<ObjectId>,
    name: String,
    color: String,
    #[serde(default)]
    privacy_level: PrivacyLevel,
    list_item_order: Option<Vec<ObjectId>>,

    #[serde(with = "bson::serde_helpers::chrono_datetime_as_bson_datetime")]
    created_at: chrono::DateTime<Utc>,
    #[serde(with = "bson::serde_helpers::chrono_datetime_as_bson_datetime")]
    updated_at: chrono::DateTime<Utc>,
}

#[derive(Serialize, Deserialize, Debug, Default)]
#[serde(rename_all = "snake_case")]
enum PrivacyLevel {
    #[default]
    Public,
    Private,
}

#[derive(Serialize, Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
struct ListItem {
    #[serde(rename = "_id", skip_serializing_if = "Option::is_none")]
    id: Option<ObjectId>,
    list_id: Option<ObjectId>,
    name: String,
    link: Option<String>,
    claimer_id: Option<ObjectId>,
    claimer_string: Option<String>,
    #[serde(with = "bson::serde_helpers::chrono_datetime_as_bson_datetime")]
    removed_at: chrono::DateTime<Utc>,

    #[serde(with = "bson::serde_helpers::chrono_datetime_as_bson_datetime")]
    created_at: chrono::DateTime<Utc>,
    #[serde(with = "bson::serde_helpers::chrono_datetime_as_bson_datetime")]
    updated_at: chrono::DateTime<Utc>,
}

#[derive(Serialize, Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
struct Friendship {
    #[serde(rename = "_id", skip_serializing_if = "Option::is_none")]
    id: Option<ObjectId>,
    user_id: ObjectId,
    friend_id: ObjectId,

    #[serde(with = "bson::serde_helpers::chrono_datetime_as_bson_datetime")]
    created_at: chrono::DateTime<Utc>,
    #[serde(with = "bson::serde_helpers::chrono_datetime_as_bson_datetime")]
    updated_at: chrono::DateTime<Utc>,
}

#[derive(Serialize, Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
struct RefreshToken {
    #[serde(rename = "_id", skip_serializing_if = "Option::is_none")]
    id: Option<ObjectId>,
    user_id: ObjectId,
    jwt: String,
    device: Device,

    #[serde(with = "bson::serde_helpers::chrono_datetime_as_bson_datetime")]
    created_at: chrono::DateTime<Utc>,
    #[serde(with = "bson::serde_helpers::chrono_datetime_as_bson_datetime")]
    updated_at: chrono::DateTime<Utc>,
}

#[derive(Serialize, Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
struct Device {
    browser: String,
    os: String,
    platform: String,
    ip_address: String,
    location: Location,
}

#[derive(Serialize, Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
struct Location {
    country_code: String,
    country_name: String,
    city: String,
    state: String,
    latitude: String,
    longitude: String,
}
