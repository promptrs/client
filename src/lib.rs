#[allow(warnings)]
mod bindings;

use a2httpc::Error;
use a2httpc::body::Json;
use a2httpc::header::CONTENT_TYPE;
use a2httpc::{ResponseReader, TextReader};
use either::IntoEither;
use serde::ser::SerializeSeq;
use serde::{Deserialize, Serialize, Serializer};
use std::collections::HashMap;
use std::io::{self, BufRead, BufReader, Lines, Write};
use std::iter::StepBy;
use std::time::Duration;

use bindings::exports::promptrs::client::completion::{
	self, Guest, Params as HostParams, Request, Response,
};

struct Component;

impl Guest for Component {
	fn receive(payload: Request) -> Result<Response, String> {
		payload.chat_completion().map_err(|err| err.to_string())
	}
}

impl Request {
	pub fn chat_completion(&self) -> Result<Response, Error> {
		let mut tool_calls = Vec::new();
		let text = self
			.stream()?
			.into_iter()
			.fold(String::new(), |acc, chunk| {
				let Ok(chunk) = chunk else {
					return acc;
				};

				let Some(Choice {
					delta: Delta {
						content,
						tool_calls: tcs,
					},
				}) = chunk.choices.into_iter().next()
				else {
					return acc;
				};

				if let Some(tcs) = tcs {
					tool_calls.extend(tcs.into_iter().map(|tc| completion::ToolCall {
						name: tc.function.name,
						arguments: tc.function.arguments,
					}));
				}

				let Some(text) = content else { return acc };
				print!("{}", text);
				_ = io::stdout().flush();
				acc + text.as_str()
			});
		println!("\n[DONE]");

		Ok(Response { text, tool_calls })
	}

	fn stream(&self) -> Result<ChatCompletionStream, Error> {
		let (status, _, reader) = a2httpc::post(self.base_url.to_string() + "/v1/chat/completions")
			.read_timeout(Duration::from_secs(600))
			.into_either(self.api_key.is_some())
			.right_or_else(|req| req.bearer_auth(self.api_key.as_ref().as_slice()[0]))
			.header(CONTENT_TYPE, "application/json")
			.body(Json(Compl(&self.body)))
			.send()?
			.split();
		if !status.is_success() {
			let resp = reader.text()?;
			return Err(Error::from(io::Error::new(
				io::ErrorKind::ConnectionRefused,
				format!("Status Code: {status}\nResponse: {resp}"),
			)));
		}

		Ok(ChatCompletionStream(
			BufReader::new(TextReader::new(reader, a2httpc::charsets::UTF_8))
				.lines()
				.step_by(2),
		))
	}
}

pub struct ChatCompletionStream(StepBy<Lines<BufReader<TextReader<ResponseReader>>>>);

impl Iterator for ChatCompletionStream {
	type Item = Result<ChatCompletionChunk, io::Error>;

	fn next(&mut self) -> Option<Self::Item> {
		self.0
			.next()
			.and_then(|s| s.and_then(next_chunk).transpose())
	}
}

fn next_chunk(mut s: String) -> Result<Option<ChatCompletionChunk>, io::Error> {
	if s.find("[DONE]").is_some() {
		return Ok(None);
	}
	if let Some(i) = s.find('{') {
		s = s.split_off(i);
	}
	serde_json::from_str(&s)
		.map_err(|err| io::Error::new(io::ErrorKind::InvalidData, format!("Malformed JSON: {err}")))
}

#[derive(Serialize)]
struct Compl<'p>(#[serde(with = "Params")] &'p HostParams);

#[derive(Serialize)]
#[serde(remote = "HostParams")]
struct Params {
	model: String,
	temperature: Option<f64>,
	top_p: Option<f64>,
	#[serde(serialize_with = "serialize_messages")]
	messages: Vec<completion::Message>,
	stream: bool,
}

fn serialize_messages<S: Serializer>(
	messages: &Vec<completion::Message>,
	serializer: S,
) -> Result<S::Ok, S::Error> {
	let mut seq = serializer.serialize_seq(Some(messages.len()))?;

	let mut messages = messages.iter();
	let mut curr = messages.next();

	loop {
		while let Some(
			completion::Message::ToolCall((assistant, tool))
			| completion::Message::Status((assistant, tool)),
		) = curr
		{
			seq.serialize_element(&HashMap::from([
				("role", "assistant"),
				("content", &assistant),
			]))?;
			seq.serialize_element(&HashMap::from([("role", "tool"), ("content", &tool)]))?;
			curr = messages.next();
		}

		let Some(message) = curr else { break };

		let (role, content) = match message {
			completion::Message::System(content) => ("system", content),
			completion::Message::User(content) => ("user", content),
			completion::Message::Assistant(content) => ("assistant", content),
			_ => unreachable!(),
		};
		let map = HashMap::from([("role", role), ("content", content)]);
		seq.serialize_element(&map)?;

		curr = messages.next();
	}
	seq.end()
}

#[derive(Debug, Deserialize)]
pub struct ChatCompletionChunk {
	pub choices: Vec<Choice>,
}

#[derive(Debug, Deserialize)]
pub struct Choice {
	#[serde(alias = "message")]
	pub delta: Delta,
}

#[derive(Debug, Deserialize)]
pub struct Delta {
	pub content: Option<String>,
	pub tool_calls: Option<Vec<ToolCall>>,
}

#[derive(Debug, Deserialize)]
pub struct ToolCall {
	function: Function,
}

#[derive(Debug, Deserialize)]
pub struct Function {
	name: String,
	arguments: String,
}

bindings::export!(Component with_types_in bindings);
