use std::{collections::HashMap, convert::Infallible};

use askama::Template;
use bson::oid::ObjectId;
use chrono::Utc;
use mongodb::{bson::doc, options::ClientOptions, Client, Collection};
use serde::{Deserialize, Serialize};
use tokio_stream::StreamExt;
use warp::http::header::{HeaderMap, HeaderValue};
use warp::{reject::Rejection, reply::Reply, Filter};

mod secrets;

// Bluu Next font is kinda nice
// cargo watch -x run

#[tokio::main]
async fn main() -> mongodb::error::Result<()> {
    let client_options = ClientOptions::parse(secrets::MONGO_URL).await?;
    let client = Client::with_options(client_options)?;
    let database = client.database("test");
    let users = database.collection::<User>("users");
    let lists = database.collection::<List>("lists");
    let list_items = database.collection::<ListItem>("listitems");
    let friendships = database.collection::<Friendship>("friendships");

    database.run_command(doc! {"ping": 1}, None).await?;
    println!("Pinged your deployment. You successfully connected to MongoDB!");
    
    let mut headers = HeaderMap::new();
    headers.insert(
        "Cache-Control",
        HeaderValue::from_static("private, max-age=3"),
    );


    let lists_page_route = warp::path!("lists")
        .and(with_lists(lists.clone()))
        .and(with_friendships(friendships.clone()))
        .and(with_users(users.clone()))
        .and_then(lists_handler)
        .with(warp::reply::with::headers(headers.clone()));

    let list_route = warp::path!("lists" / String)
        .and(with_lists(lists.clone()))
        .and(with_users(users.clone()))
        .and(with_friendships(friendships.clone()))
        .and(with_list_items(list_items.clone()))
        .and(warp::header::optional::<String>("HX-Request"))
        .and_then(single_list_handler)
        .with(warp::reply::with::headers(headers));

    warp::serve(list_route.or(lists_page_route))
        .run(([0, 0, 0, 0, 0, 0, 0, 0], 3030))
        .await;

    Ok(())
}

fn with_lists(
    lists: Collection<List>,
) -> impl Filter<Extract = (Collection<List>,), Error = Infallible> + Clone {
    warp::any().map(move || lists.clone())
}

fn with_friendships(
    friendships: Collection<Friendship>,
) -> impl Filter<Extract = (Collection<Friendship>,), Error = Infallible> + Clone {
    warp::any().map(move || friendships.clone())
}

fn with_users(
    users: Collection<User>,
) -> impl Filter<Extract = (Collection<User>,), Error = Infallible> + Clone {
    warp::any().map(move || users.clone())
}

fn with_list_items(
    list_items: Collection<ListItem>,
) -> impl Filter<Extract = (Collection<ListItem>,), Error = Infallible> + Clone {
    warp::any().map(move || list_items.clone())
}

type WarpResult<T> = std::result::Result<T, Rejection>;

async fn lists_handler(
    lists: Collection<List>,
    friendships: Collection<Friendship>,
    users: Collection<User>,
) -> WarpResult<ListsPage> {
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

    Ok(ListsPage {
        my_lists,
        friends_lists,
    })
}

async fn single_list_handler(
    list_id: String,
    lists: Collection<List>,
    users: Collection<User>,
    friendships: Collection<Friendship>,
    list_items: Collection<ListItem>,
    hx_request: Option<String>,
) -> WarpResult<impl Reply> {
    let list = lists
        .find_one(doc! {"_id": ObjectId::parse_str(list_id).unwrap()}, None)
        .await
        .unwrap()
        .ok_or(warp::reject())?;

    let user = match list.user_id {
        Some(id) => Ok(users
            .find_one(doc! {"_id": id}, None)
            .await
            .unwrap()
            .ok_or(warp::reject())?),
        None => Err(warp::reject()),
    }?;

    let list_items = list_items
        .find(doc! {"listId": list.id.unwrap()}, None)
        .await
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .await
        .unwrap();

    match hx_request.as_deref() {
        Some("true") => Ok(ViewType::Partial(SingleListPartial {
            list,
            user,
            list_items,
        })),
        _ => {
            let ListsPage {
                my_lists,
                friends_lists,
            } = lists_handler(lists, friendships, users).await?;
            // ^^ there is parallelization to be had here
            Ok(ViewType::Page(ListsPageViewSingleList {
                list,
                user,
                list_items,
                my_lists,
                friends_lists,
            }))
        }
    }
}

enum ViewType {
    Partial(SingleListPartial),
    Page(ListsPageViewSingleList),
}

impl Reply for ViewType {
    fn into_response(self) -> warp::reply::Response {
        match self {
            Self::Partial(t) => {
                let body = t
                    .render()
                    .unwrap_or_else(|e| format!("Failed to render template: {}", e));
                warp::reply::html(body).into_response()
            }
            Self::Page(t) => {
                let body = t
                    .render()
                    .unwrap_or_else(|e| format!("Failed to render template: {}", e));
                warp::reply::html(body).into_response()
            }
        }
    }
}

#[derive(Template)]
#[template(path = "list_partial.html")]
struct SingleListPartial {
    list: List,
    list_items: Vec<ListItem>,
    user: User,
}

// #[derive(Template)]
// #[template(path = "list.html")]
// struct SingleListPage {
//     list: List,
//     list_items: Vec<ListItem>,
//     user: User,
// }

#[derive(Template)]
#[template(path = "lists.html")]
struct ListsPage {
    my_lists: Vec<List>,
    friends_lists: Vec<(User, Vec<List>)>,
}

#[derive(Template)]
#[template(path = "list.html")]
struct ListsPageViewSingleList {
    my_lists: Vec<List>,
    friends_lists: Vec<(User, Vec<List>)>,
    list: List,
    list_items: Vec<ListItem>,
    user: User,
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
    // #[serde(with = "bson::serde_helpers::chrono_datetime_as_bson_datetime")]
    #[serde(skip_serializing_if = "Option::is_none")]
    removed_at: Option<OptionalDateTime>,

    #[serde(with = "bson::serde_helpers::chrono_datetime_as_bson_datetime")]
    created_at: chrono::DateTime<Utc>,
    #[serde(with = "bson::serde_helpers::chrono_datetime_as_bson_datetime")]
    updated_at: chrono::DateTime<Utc>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(transparent)]
struct OptionalDateTime {
    #[serde(with = "bson::serde_helpers::chrono_datetime_as_bson_datetime")]
    value: chrono::DateTime<Utc>,
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
