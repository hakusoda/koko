pub async fn publish_message(experience_id: impl Into<u64>, api_key: impl Into<String>, topic: impl Into<String>, message: impl Into<String>) {
	println!("{}", reqwest::Client::new()
		.post(format!("https://apis.roblox.com/messaging-service/v1/universes/{}/topics/{}", experience_id.into(), topic.into()))
		.json(&serde_json::json!({
			"message": message.into()
		}))
		.header("x-api-key", api_key.into())
		.header("content-type", "application/json")
		.send()
		.await
		.unwrap()
		.status()
	);
}