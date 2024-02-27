use std::{
	sync::Arc,
	time::Duration,
	collections::HashMap
};
use uuid::Uuid;
use rand::{ distributions::Alphanumeric, Rng };
use tokio::{
	sync::RwLock,
	task::JoinHandle
};
use serde::{ ser::SerializeSeq, Deserialize, Serialize, Serializer };
use chrono::{ serde::ts_seconds, Utc, DateTime };
use actix_web::{
	web,
	http::{ header::ContentType, StatusCode },
	middleware::Logger,
	App, HttpServer, HttpResponse
};
use tokio_util::sync::CancellationToken;
use derive_more::{ Error, Display };
use simple_logger::SimpleLogger;

mod routes;
mod roblox;
mod database;

#[derive(Clone, Debug, Serialize)]
pub struct Server {
	pub id: Uuid,
	pub country: Option<String>,
	pub place_id: u64,
	pub place_version: u64,
	pub private_server_id: Option<String>,
	pub has_requested_actions: bool,

	#[serde(serialize_with = "ts_iso")]
	pub created_at: DateTime<Utc>,

	#[serde(serialize_with = "serialize_hashmap_to_vec")]
	pub players: HashMap<u64, ServerPlayer>,
	pub experience: database::Experience,

	#[serde(skip_serializing)]
	pub connection_token: String,

	#[serde(skip)]
	pub ack_token: Option<String>,

	#[serde(skip)]
	pub last_ack_at: DateTime<Utc>,

	#[serde(skip)]
	pub last_ack_sent_at: DateTime<Utc>,

	#[serde(skip)]
	pub secret_topic: String
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ServerPlayer {
	pub id: u64,
	pub name: String,
	pub username: String,
	pub has_verified_badge: bool,

	#[serde(serialize_with = "ts_iso", deserialize_with = "ts_seconds::deserialize")]
	pub joined_at: DateTime<Utc>,
	pub joined_via_user: u64
}

impl ServerPlayer {
	pub fn joined_via_user(&self) -> Option<u64> {
		if self.joined_via_user == 0 { None } else { Some(self.joined_via_user) }
	}
}

pub struct PendingConnection {
	pub place_id: u64,
	pub server_id: Uuid,
	pub experience: database::Experience,
	pub secret_topic: String,
	pub connection_token: String
}

pub struct AppState {
	pub servers: Arc<Servers>,
	pub pending_connections: RwLock<HashMap<String, PendingConnection>>
}

pub type Servers = RwLock<HashMap<String, Server>>;

pub fn init_servers() -> (Arc<Servers>, JoinHandle<()>, CancellationToken) {
	let servers = Arc::new(Servers::default());

	let job_cancel = CancellationToken::new();
	(
		Arc::clone(&servers),
		tokio::spawn(spawn_servers_job(
			Arc::clone(&servers),
			job_cancel.clone()
		)),
		job_cancel
	)
}

async fn spawn_servers_job(servers: Arc<Servers>, stop_signal: CancellationToken) {
	loop {
		if let Ok(mut servers) = servers.try_write() {
			let mut remove_servers: Vec<String> = vec![];
			for (key, server) in servers.iter_mut() {
				if server.ack_token.is_some() {
					if Utc::now().signed_duration_since(server.last_ack_sent_at).num_seconds() >= 10 {
						println!("server {} did not acknowledge, deeming as frozen or closed.", server.id);
						remove_servers.push(key.clone());
					}
				} else if Utc::now().signed_duration_since(server.last_ack_at).num_seconds() >= 10 {
					//println!("sending ack request to server {}, excepting response within 10 seconds.", server.id);
					let ack_token: String = rand::thread_rng()
						.sample_iter(&Alphanumeric)
						.take(64)
						.map(char::from)
						.collect();
					roblox::publish_message(server.experience.id, &server.experience.open_cloud_api_key, format!("{}1", server.secret_topic), &ack_token).await;
					
					server.ack_token = Some(ack_token);
					server.last_ack_sent_at = Utc::now();
				}
			}
			
			for key in remove_servers {
				servers.remove(&key);
			}
		}

		tokio::select! {
            _ = tokio::time::sleep(Duration::from_secs(10)) => {
                continue;
            }

            _ = stop_signal.cancelled() => {
                log::info!("gracefully shutting down servers job");
                break;
            }
        };
	}
}

#[actix_web::main]
async fn main() -> std::io::Result<()> {
	SimpleLogger::new()
		.with_level(log::LevelFilter::Info)
		.env()
		.init()
		.unwrap();

	let (servers, servers_job_handle, servers_job_cancel) = init_servers();

	let data = web::Data::new(AppState {
		servers: Arc::clone(&servers),
		pending_connections: RwLock::new(HashMap::new())
	});
	HttpServer::new(move ||
		App::new()
			.wrap(Logger::new("%r  →  %s, %b bytes, took %Dms"))
			.app_data(data.clone())
			.configure(routes::configure)
	)
		.bind(("127.0.0.1", 8081))?
		.run()
		.await?;

	servers_job_cancel.cancel();
	servers_job_handle.await.unwrap();

	Ok(())
}

#[derive(Debug, Display, Error)]
pub enum ApiError {
	#[display(fmt = "internal_error")]
	InternalError,

	#[display(fmt = "invalid_request")]
	GenericInvalidRequest,

	#[display(fmt = "missing_headers")]
	MissingHeaders,

	#[display(fmt = "invalid_api_key")]
	InvalidApiKey,

	#[display(fmt = "invalid_signature")]
	InvalidSignature,

	#[display(fmt = "server_already_connected")]
	ServerAlreadyConnected,

	#[display(fmt = "resource_not_found")]
	ResourceNotFound,

	#[display(fmt = "invalid_request_origin")]
	InvalidRequestOrigin,

	#[display(fmt = "not_implemented")]
	NotImplemented,

	#[display(fmt = "too_many_requests")]
	TooManyRequests
}

impl actix_web::error::ResponseError for ApiError {
	fn error_response(&self) -> HttpResponse {
		HttpResponse::build(self.status_code())
			.insert_header(ContentType::json())
			.body(format!(r#"{{
				"error": "{}"
			}}"#, self.to_string()))
	}

	fn status_code(&self) -> StatusCode {
		match *self {
			ApiError::InternalError => StatusCode::INTERNAL_SERVER_ERROR,
			ApiError::GenericInvalidRequest |
			ApiError::ServerAlreadyConnected |
			ApiError::MissingHeaders => StatusCode::BAD_REQUEST,
			ApiError::InvalidApiKey |
			ApiError::InvalidSignature |
			ApiError::InvalidRequestOrigin => StatusCode::FORBIDDEN,
			ApiError::ResourceNotFound => StatusCode::NOT_FOUND,
			ApiError::NotImplemented => StatusCode::NOT_IMPLEMENTED,
			ApiError::TooManyRequests => StatusCode::TOO_MANY_REQUESTS
		}
	}
}

pub type ApiResult<T> = actix_web::Result<T, ApiError>;

pub fn serialize_hashmap_to_vec<S: Serializer, K, V: Serialize>(map: &HashMap<K, V>, serializer: S) -> Result<S::Ok, S::Error> {
	let mut sequence = serializer.serialize_seq(Some(map.len()))?;
	for (_,value) in map {
		sequence.serialize_element(value)?;
	}
	sequence.end()
}

pub fn ts_iso<S: Serializer>(date: &DateTime<Utc>, serializer: S) -> Result<S::Ok, S::Error> {
	serializer.serialize_str(&format!("{}", date.format("%+")))
}