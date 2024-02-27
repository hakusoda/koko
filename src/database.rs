use serde::{ Serialize, Deserialize };
use once_cell::sync::Lazy;
use postgrest::Postgrest;

pub const DATABASE: Lazy<Postgrest> = Lazy::new(|| {
	let key = env!("SUPABASE_API_KEY");
	Postgrest::new("https://hakumi.supabase.co/rest/v1")
		.insert_header("apikey", key)
		.insert_header("authorization", format!("Bearer {}", key))
});

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Experience {
	pub id: u64,
	
	#[serde(skip_serializing)]
	pub open_cloud_api_key: String
}

pub async fn get_experience_by_id(experience_id: u64) -> Experience {
	serde_json::from_str(&DATABASE.from("koko_experiences")
		.select("id,open_cloud_api_key")
		.eq("id", experience_id.to_string())
		.limit(1)
		.single()
		.execute()
		.await
		.unwrap()
		.text()
		.await
		.unwrap()
	).unwrap()
}

pub async fn get_experience_by_api_key(api_key: impl Into<String>) -> Experience {
	serde_json::from_str(&DATABASE.from("koko_experiences")
		.select("id,open_cloud_api_key")
		.eq("api_key", api_key.into())
		.limit(1)
		.single()
		.execute()
		.await
		.unwrap()
		.text()
		.await
		.unwrap()
	).unwrap()
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ExperienceServerAction {
	pub id: String
}

pub async fn get_experience_server_actions(experience_id: u64) -> Vec<ExperienceServerAction> {
	let t = &DATABASE.from("koko_experience_server_actions")
	.select("id")
	.eq("experience_id", experience_id.to_string())
	.execute()
	.await
	.unwrap()
	.text()
	.await
	.unwrap();
println!("{t}");
	serde_json::from_str(t
	).unwrap()
}