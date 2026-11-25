use std::future::Future;

use serde_json::{json, Value};

use crate::protocol::*;

/// Who is calling, read from the pipe client's token.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Caller {
    /// The user's SID; empty when Windows would not say.
    pub sid: String,
    /// An elevated (administrator) token.
    pub elevated: bool,
}

/// The methods behind the pipe. The service implements this.
pub trait Handler: Send + Sync + 'static {
    fn call(
        &self,
        caller: &Caller,
        method: &str,
        params: Value,
    ) -> impl Future<Output = Result<Value, RpcError>> + Send;
}

/// Per-connection state machine: `hello` first, then methods.
#[derive(Default)]
pub struct Connection {
    greeted: bool,
    pub caller: Caller,
}

impl Connection {
    pub async fn handle<H: Handler>(&mut self, raw: &[u8], handler: &H) -> Response {
        let req: Request = match serde_json::from_slice(raw) {
            Ok(r) => r,
            Err(e) => return Response::err(None, PARSE_ERROR, e.to_string()),
        };
        let id = req.id;
        if req.jsonrpc != "2.0" {
            return Response::err(Some(id), INVALID_REQUEST, "jsonrpc must be \"2.0\"");
        }
        if req.method != "hello" {
            if !self.greeted {
                return Response::err(Some(id), HELLO_REQUIRED, "call hello first");
            }
            return match handler.call(&self.caller, &req.method, req.params).await {
                Ok(v) => Response::ok(id, v),
                Err(e) => Response::err(Some(id), e.code, e.message),
            };
        }
        let hello: Hello = match serde_json::from_value(req.params) {
            Ok(h) => h,
            Err(e) => return Response::err(Some(id), INVALID_PARAMS, e.to_string()),
        };
        if hello.protocol != PROTOCOL_VERSION {
            return Response::err(
                Some(id),
                VERSION_MISMATCH,
                format!(
                    "client speaks protocol {}, service speaks {PROTOCOL_VERSION}; \
                     update the client or the service",
                    hello.protocol
                ),
            );
        }
        self.greeted = true;
        Response::ok(
            id,
            json!({ "protocol": PROTOCOL_VERSION, "service_version": env!("CARGO_PKG_VERSION") }),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fake;
    impl Handler for Fake {
        async fn call(&self, _: &Caller, method: &str, params: Value) -> Result<Value, RpcError> {
            match method {
                "echo" => Ok(params),
                "boom" => Err(RpcError::new(INTERNAL_ERROR, "GetSystemPowerStatus failed")),
                other => Err(RpcError::new(
                    METHOD_NOT_FOUND,
                    format!("unknown method {other}"),
                )),
            }
        }
    }

    async fn call(c: &mut Connection, method: &str, params: Value) -> Response {
        let raw = serde_json::to_vec(&Request::new(7, method, params)).unwrap();
        c.handle(&raw, &Fake).await
    }

    fn code(r: &Response) -> i64 {
        r.error.as_ref().expect("expected an error").code
    }

    const HELLO: &str = r#"{"protocol": 1, "role": "cli"}"#;

    fn hello() -> Value {
        serde_json::from_str(HELLO).unwrap()
    }

    #[tokio::test]
    async fn methods_require_hello_first() {
        let mut c = Connection::default();
        assert_eq!(code(&call(&mut c, "echo", json!({})).await), HELLO_REQUIRED);
        assert!(call(&mut c, "hello", hello()).await.error.is_none());
        let r = call(&mut c, "echo", json!({"x": 1})).await;
        assert_eq!((r.id, r.result), (Some(7), Some(json!({"x": 1}))));
    }

    #[tokio::test]
    async fn version_mismatch_is_explained() {
        let mut c = Connection::default();
        let r = call(&mut c, "hello", json!({"protocol": 2, "role": "ui"})).await;
        assert_eq!(code(&r), VERSION_MISMATCH);
        assert!(r.error.unwrap().message.contains("protocol 2"));
        // A failed hello does not open the connection.
        assert_eq!(code(&call(&mut c, "echo", json!({})).await), HELLO_REQUIRED);
    }

    #[tokio::test]
    async fn bad_input_gets_json_rpc_errors() {
        let mut c = Connection::default();
        assert_eq!(code(&c.handle(b"not json", &Fake).await), PARSE_ERROR);
        let no_protocol = call(&mut c, "hello", json!({"role": "cli"})).await;
        assert_eq!(code(&no_protocol), INVALID_PARAMS);
        call(&mut c, "hello", hello()).await;
        assert_eq!(
            code(&call(&mut c, "nope", json!(null)).await),
            METHOD_NOT_FOUND
        );
        let wrong = br#"{"jsonrpc":"1.0","id":1,"method":"hello"}"#;
        assert_eq!(code(&c.handle(wrong, &Fake).await), INVALID_REQUEST);
    }

    #[tokio::test]
    async fn handler_errors_pass_through() {
        let mut c = Connection::default();
        call(&mut c, "hello", hello()).await;
        let r = call(&mut c, "boom", json!({})).await;
        assert_eq!(code(&r), INTERNAL_ERROR);
        assert!(r.error.unwrap().message.contains("GetSystemPowerStatus"));
    }
}
