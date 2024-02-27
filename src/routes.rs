use std::collections::HashMap;
use uuid::Uuid;
use rand::{ distributions::Alphanumeric, Rng };
use serde::Deserialize;
use chrono::Utc;
use actix_web::{
	web::{ self, Json, Path, ServiceConfig },
	Responder, HttpRequest, HttpResponse,
	get, put, post, delete
};

use crate::{
	roblox,
	database::{ ExperienceServerAction, get_experience_by_id, get_experience_by_api_key, get_experience_server_actions },
	Server, AppState, ApiError, ApiResult, ServerPlayer, PendingConnection
};

const API_KEY: &str = env!("API_KEY");
const CARGO_PKG_VERSION: &str = env!("CARGO_PKG_VERSION");

pub fn configure(cfg: &mut ServiceConfig) {
	cfg
		.service(index)
		.service(create_server)
		.service(verify_server)
		.service(get_server_actions)
		.service(set_server_players)
		.service(set_server_player)
		.service(remove_server_player)
		.service(remove_server)
		.service(server_ack)
		.service(get_experience_servers)
		.service(trigger_server_action);
		//.service(get_roblox_server);
}

#[get("/")]
async fn index() -> impl Responder {
	HttpResponse::Ok().body(format!("hello from KoKo v{CARGO_PKG_VERSION}!\nhttps://github.com/hakusoda/koko"))
}

#[derive(Deserialize)]
struct CreateServerPayload {
	pub server_id: Uuid,
	pub secret_topic: String
}

#[post("/server")]
async fn create_server(request: HttpRequest, body: Json<CreateServerPayload>, state: web::Data<AppState>) -> ApiResult<HttpResponse> {
	// TODO: implement new check
	/*let request_ip = request.connection_info().realip_remote_addr().map(|x| x.to_string()).unwrap_or(request.peer_addr().unwrap().to_string());
	if state.servers.read().await.contains_key(&request_ip) || state.pending_connections.read().await.contains_key(&request_ip) {
		return Err(ApiError::ServerAlreadyConnected);
	}*/

	let api_key = match request.headers().get("x-api-key") {
		Some(x) => x.to_str().unwrap(),
		None => return Err(ApiError::InvalidApiKey)
	};
	let experience = get_experience_by_api_key(api_key).await;

	let place_id = get_place_id_from_request(&request)?;
	let connection_token: String = rand::thread_rng()
		.sample_iter(&Alphanumeric)
		.take(64)
		.map(char::from)
		.collect();

	println!("{}", body.secret_topic);
	roblox::publish_message(experience.id, &experience.open_cloud_api_key, format!("{}0", body.secret_topic), &connection_token).await;

	println!("pending verification using {connection_token}");
	state.pending_connections.write().await.insert(connection_token.clone(), PendingConnection {
		place_id,
		server_id: body.server_id,
		experience,
		secret_topic: body.secret_topic.clone(),
		connection_token
	});

	Ok(HttpResponse::Ok().finish())
}

#[derive(Deserialize)]
struct VerifyServerPayload {
	pub place_version: u64,
	pub private_server_id: String
}

#[post("/server/verify")]
async fn verify_server(request: HttpRequest, body: Json<VerifyServerPayload>, state: web::Data<AppState>) -> ApiResult<HttpResponse> {
	/*let request_ip = request.connection_info().realip_remote_addr().map(|x| x.to_string()).unwrap_or(request.peer_addr().unwrap().to_string());
	if state.servers.read().await.contains_key(&request_ip) {
		println!("attempted to verify, but already connected");
		return Err(ApiError::ServerAlreadyConnected);
	}
	println!("verify attempt from {request_ip}");*/
	let token = get_token_from_request(&request)?;
	let place_id = get_place_id_from_request(&request)?;

	let mut connections = state.pending_connections.write().await;
	if let Some(pending) = connections.remove(&token) {
		let payload = body.0;
		let server = Server {
			id: pending.server_id,
			players: HashMap::new(),
			country: request.headers().get("cf-ipcountry").map(|x| x.to_str().unwrap().to_string()),
			place_id,
			created_at: Utc::now(),
			experience: pending.experience,
			secret_topic: pending.secret_topic,
			place_version: payload.place_version,
			connection_token: pending.connection_token,
			private_server_id: if payload.private_server_id.is_empty() { None } else { Some(payload.private_server_id) },
			has_requested_actions: false,
			
			ack_token: None,
			last_ack_at: Utc::now(),
			last_ack_sent_at: Utc::now()
		};
		state.servers.write().await.insert(token, server);

		println!("server verified {}", pending.server_id);
		return Ok(HttpResponse::Ok().finish());
	}
	println!("server not pending");
	Err(ApiError::GenericInvalidRequest)
}

#[get("/server/actions")]
async fn get_server_actions(request: HttpRequest, state: web::Data<AppState>) -> ApiResult<Json<Vec<ExperienceServerAction>>> {
	if let Some(server) = state.servers.write().await.get_mut(&get_token_from_request(&request)?) {
		if server.has_requested_actions {
			return Err(ApiError::TooManyRequests);
		}
		server.has_requested_actions = true;

		return Ok(Json(get_experience_server_actions(server.experience.id).await));
	}
	Err(ApiError::GenericInvalidRequest)
}

#[put("/server/players")]
async fn set_server_players(request: HttpRequest, body: Json<HashMap<u64, ServerPlayer>>, state: web::Data<AppState>) -> ApiResult<HttpResponse> {
	if let Some(server) = state.servers.write().await.get_mut(&get_token_from_request(&request)?) {
		server.players = body.0;
		return Ok(HttpResponse::Ok().finish());
	}
	Err(ApiError::GenericInvalidRequest)
}

#[put("/server/player/{user_id}")]
async fn set_server_player(request: HttpRequest, body: Json<ServerPlayer>, state: web::Data<AppState>, path: Path<u64>) -> ApiResult<HttpResponse> {
	if let Some(server) = state.servers.write().await.get_mut(&get_token_from_request(&request)?) {
		server.players.insert(path.into_inner(), body.0);
		return Ok(HttpResponse::Ok().finish());
	}
	Err(ApiError::GenericInvalidRequest)
}

#[delete("/server/player/{user_id}")]
async fn remove_server_player(request: HttpRequest, state: web::Data<AppState>, path: Path<u64>) -> ApiResult<HttpResponse> {
	if let Some(server) = state.servers.write().await.get_mut(&get_token_from_request(&request)?) {
		server.players.remove(&path.into_inner());
		return Ok(HttpResponse::Ok().finish());
	}
	Err(ApiError::GenericInvalidRequest)
}

#[post("/server/ack")]
async fn server_ack(request: HttpRequest, body: String, state: web::Data<AppState>) -> ApiResult<HttpResponse> {
	if let Some(server) = state.servers.write().await.get_mut(&get_token_from_request(&request)?) {
		if server.ack_token.as_ref().is_some_and(|x| x == &body) {
			println!("server successfully acknowledged");
			server.ack_token = None;
			server.last_ack_at = Utc::now();
			return Ok(HttpResponse::Ok().finish());
		}
	}
	Err(ApiError::GenericInvalidRequest)
}

#[delete("/server")]
async fn remove_server(request: HttpRequest, state: web::Data<AppState>) -> ApiResult<HttpResponse> {
	if state.servers.write().await.remove(&get_token_from_request(&request)?).is_some() {
		return Ok(HttpResponse::Ok().finish());
	}
	Err(ApiError::GenericInvalidRequest)
}

#[get("/experience/{experience_id}/servers")]
async fn get_experience_servers(request: HttpRequest, state: web::Data<AppState>, path: Path<u64>) -> ApiResult<Json<Vec<Server>>> {
	if !request.headers().get("x-api-key").is_some_and(|x| x.to_str().unwrap() == API_KEY) {
		return Err(ApiError::InvalidApiKey);
	}
	
	let experience_id = path.into_inner();
	let servers: Vec<Server> = state.servers.read().await.iter()
		.filter(|x| x.1.experience.id == experience_id)
		.map(|x| x.1.clone())
		.collect();

	Ok(Json(servers))
}

#[derive(Deserialize)]
struct TriggerServerActionPayload {
	id: String
}

#[post("/experience/{experience_id}/trigger_action")]
async fn trigger_server_action(request: HttpRequest, body: Json<TriggerServerActionPayload>, path: Path<u64>) -> ApiResult<HttpResponse> {
	if !request.headers().get("x-api-key").is_some_and(|x| x.to_str().unwrap() == API_KEY) {
		return Err(ApiError::InvalidApiKey);
	}
	
	let experience_id = path.into_inner();
	let experience = get_experience_by_id(experience_id).await;
	roblox::publish_message(experience_id, experience.open_cloud_api_key, "koko_global_0", &body.id).await;

	Ok(HttpResponse::Ok().finish())
}

fn get_token_from_request(request: &HttpRequest) -> ApiResult<String> {
	match request.headers().get("x-connection-token") {
		Some(x) => Ok(x.to_str().unwrap().to_string()),
		None => Err(ApiError::InvalidApiKey)
	}
}

fn get_place_id_from_request(request: &HttpRequest) -> ApiResult<u64> {
	match request.headers().get("roblox-id") {
		Some(x) => Ok(x.to_str().unwrap().parse().unwrap()),
		None => return Err(ApiError::MissingHeaders)
	}
}

/*#[derive(Debug, Deserialize)]
struct UdmuxEndpoint {
	#[serde(rename = "Address")]
	address: String
}

#[derive(Debug, Deserialize)]
struct JoinScript {
	#[serde(rename = "MachineAddress")]
	machine_address: String,
	#[serde(rename = "UdmuxEndpoints")]
	udmux_endpoints: Vec<UdmuxEndpoint>
}

#[derive(Debug, Deserialize)]
struct GameJoinData {
	#[serde(rename = "joinScript")]
	join_script: JoinScript
}

impl GameJoinData {
	fn get_ipv4_address(&self) -> &String {
		//self.join_script.udmux_endpoints.first().map(|x| &x.address).unwrap()
		&self.join_script.machine_address
	}
}

#[derive(Serialize)]
struct Response {
	ipv4_address: String
}

#[get("/roblox/place/{place_id}/server/{server_id}")]
async fn get_roblox_server(path: Path<(u64, String)>) -> impl Responder {
	let (place_id, server_id) = path.into_inner();
	let data = get_roblox_server_info(place_id, server_id).await.unwrap();
	Json(Response {
		ipv4_address: data.get_ipv4_address().to_string()
	})
}

async fn get_roblox_server_info(place_id: u64, server_id: impl Into<String>) -> Option<GameJoinData> {
	let server_id = server_id.into();
	reqwest::Client::new()
		.post("https://gamejoin.roblox.com/v1/join-game-instance")
		.json(&serde_json::json!({
			"gameId": server_id,
			"placeId": place_id,
			"isTeleport": false,
			"gameJoinAttemptId": server_id
		}))
		.header("cookie", format!(".ROBLOSECURITY={}; path=/; domain=.roblox.com;", env!("ROBLOX_TOKEN")))
		.header("origin", "https://roblox.com")
		.header("referer", format!("https://roblox.com/games/{server_id}/"))
		.header("user-agent", "Roblox")
		.send()
		.await
		.unwrap()
		.json()
		.await
		.ok()
}*/