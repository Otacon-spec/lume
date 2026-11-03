use serde::{
    Deserialize, Deserializer, Serialize, Serializer, de::Error as _, ser::SerializeStruct,
};
use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct RequestId(pub i64);

impl From<i64> for RequestId {
    fn from(value: i64) -> Self {
        Self(value)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum RpcMessage {
    Request(RpcRequest),
    Response(RpcResponse),
    Notification(RpcNotification),
}

impl Serialize for RpcMessage {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match self {
            Self::Request(request) => request.serialize(serializer),
            Self::Response(response) => response.serialize(serializer),
            Self::Notification(notification) => notification.serialize(serializer),
        }
    }
}

impl<'de> Deserialize<'de> for RpcMessage {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = Value::deserialize(deserializer)?;
        let object = value
            .as_object()
            .ok_or_else(|| D::Error::custom("JSON-RPC message must be an object"))?;
        let has_method = object.contains_key("method");
        let has_id = object.contains_key("id");
        let has_result = object.contains_key("result");
        let has_error = object.contains_key("error");

        if has_method {
            if has_result || has_error {
                return Err(D::Error::custom(
                    "JSON-RPC message cannot mix method with result or error",
                ));
            }
            if has_id {
                return serde_json::from_value(value)
                    .map(Self::Request)
                    .map_err(D::Error::custom);
            }
            return serde_json::from_value(value)
                .map(Self::Notification)
                .map_err(D::Error::custom);
        }

        serde_json::from_value(value)
            .map(Self::Response)
            .map_err(D::Error::custom)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RpcRequest {
    pub jsonrpc: JsonRpcVersion,
    pub id: RequestId,
    pub method: String,
    #[serde(default, skip_serializing_if = "Value::is_null")]
    pub params: Value,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RpcNotification {
    pub jsonrpc: JsonRpcVersion,
    pub method: String,
    #[serde(default, skip_serializing_if = "Value::is_null")]
    pub params: Value,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RpcResponse {
    pub jsonrpc: JsonRpcVersion,
    pub id: RequestId,
    pub payload: RpcResponsePayload,
}

#[derive(Debug, Clone, PartialEq)]
pub enum RpcResponsePayload {
    Result(Value),
    Error(RpcError),
}

impl Serialize for RpcResponse {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut response = serializer.serialize_struct("RpcResponse", 3)?;
        response.serialize_field("jsonrpc", &self.jsonrpc)?;
        response.serialize_field("id", &self.id)?;
        match &self.payload {
            RpcResponsePayload::Result(result) => {
                response.serialize_field("result", result)?;
            }
            RpcResponsePayload::Error(error) => {
                response.serialize_field("error", error)?;
            }
        }
        response.end()
    }
}

impl<'de> Deserialize<'de> for RpcResponse {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = Value::deserialize(deserializer)?;
        let object = value
            .as_object()
            .ok_or_else(|| D::Error::custom("JSON-RPC response must be an object"))?;
        let has_result = object.contains_key("result");
        let has_error = object.contains_key("error");

        match (has_result, has_error) {
            (true, false) => {
                let wire = serde_json::from_value::<RpcResultResponseWire>(value)
                    .map_err(D::Error::custom)?;
                Ok(Self {
                    jsonrpc: wire.jsonrpc,
                    id: wire.id,
                    payload: RpcResponsePayload::Result(wire.result),
                })
            }
            (false, true) => {
                let wire = serde_json::from_value::<RpcErrorResponseWire>(value)
                    .map_err(D::Error::custom)?;
                Ok(Self {
                    jsonrpc: wire.jsonrpc,
                    id: wire.id,
                    payload: RpcResponsePayload::Error(wire.error),
                })
            }
            (true, true) => Err(D::Error::custom(
                "JSON-RPC response must contain exactly one of result or error",
            )),
            (false, false) => Err(D::Error::custom(
                "JSON-RPC response must contain exactly one of result or error",
            )),
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RpcResultResponseWire {
    jsonrpc: JsonRpcVersion,
    id: RequestId,
    result: Value,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RpcErrorResponseWire {
    jsonrpc: JsonRpcVersion,
    id: RequestId,
    error: RpcError,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RpcError {
    pub code: i64,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum JsonRpcVersion {
    #[serde(rename = "2.0")]
    V2,
}

impl RpcMessage {
    pub fn request(id: impl Into<RequestId>, method: impl Into<String>, params: Value) -> Self {
        Self::Request(RpcRequest {
            jsonrpc: JsonRpcVersion::V2,
            id: id.into(),
            method: method.into(),
            params,
        })
    }

    pub fn notification(method: impl Into<String>, params: Value) -> Self {
        Self::Notification(RpcNotification {
            jsonrpc: JsonRpcVersion::V2,
            method: method.into(),
            params,
        })
    }

    pub fn success(id: impl Into<RequestId>, result: Value) -> Self {
        Self::Response(RpcResponse {
            jsonrpc: JsonRpcVersion::V2,
            id: id.into(),
            payload: RpcResponsePayload::Result(result),
        })
    }

    pub fn failure(
        id: impl Into<RequestId>,
        code: i64,
        message: impl Into<String>,
        data: Option<Value>,
    ) -> Self {
        Self::Response(RpcResponse {
            jsonrpc: JsonRpcVersion::V2,
            id: id.into(),
            payload: RpcResponsePayload::Error(RpcError {
                code,
                message: message.into(),
                data,
            }),
        })
    }
}
