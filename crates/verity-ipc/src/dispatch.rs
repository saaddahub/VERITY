use serde_json::json;
use verity_core::SystemInfoApi;

use crate::protocol::*;

/// Per-connection state machine: `hello` first, then methods.
#[derive(Default)]
pub struct Connection {
    greeted: bool,
}

impl Connection {
    pub fn handle(&mut self, raw: &[u8], api: &dyn SystemInfoApi) -> Response {
        let req: Request = match serde_json::from_slice(raw) {
            Ok(r) => r,
            Err(e) => return Response::err(None, PARSE_ERROR, e.to_string()),
        };
        let id = req.id;
        if req.jsonrpc != "2.0" {
            return Response::err(Some(id), INVALID_REQUEST, "jsonrpc must be \"2.0\"");
        }
        if !self.greeted && req.method != "hello" {
            return Response::err(Some(id), HELLO_REQUIRED, "call hello first");
        }
        match req.method.as_str() {
            "hello" => {
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
            "system.info" => match api.system_info() {
                Ok(info) => match serde_json::to_value(info) {
                    Ok(v) => Response::ok(id, v),
                    Err(e) => Response::err(Some(id), INTERNAL_ERROR, e.to_string()),
                },
                Err(e) => Response::err(Some(id), INTERNAL_ERROR, e.to_string()),
            },
            other => Response::err(
                Some(id),
                METHOD_NOT_FOUND,
                format!("unknown method {other}"),
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use verity_core::*;

    struct Fake(Result<SystemInfo, OsError>);
    impl SystemInfoApi for Fake {
        fn system_info(&self) -> Result<SystemInfo, OsError> {
            self.0.clone()
        }
    }

    fn sample() -> SystemInfo {
        SystemInfo {
            windows: WindowsVersion {
                major: 10,
                build: 26100,
                ubr: 1,
                display_version: Some("24H2".into()),
            },
            cpu: Cpu {
                model: "Test CPU".into(),
                logical: vec![],
            },
            gpus: vec![],
            displays: vec![],
            power: PowerState::from_raw(1, 128, 255),
            power_scheme: PowerScheme {
                guid: "g".into(),
                name: None,
            },
        }
    }

    fn call(c: &mut Connection, api: &dyn SystemInfoApi, method: &str, params: Value) -> Response {
        let raw = serde_json::to_vec(&Request::new(7, method, params)).unwrap();
        c.handle(&raw, api)
    }

    use serde_json::Value;

    fn code(r: &Response) -> i64 {
        r.error.as_ref().expect("expected an error").code
    }

    #[test]
    fn methods_require_hello_first() {
        let api = Fake(Ok(sample()));
        let mut c = Connection::default();
        assert_eq!(
            code(&call(&mut c, &api, "system.info", json!({}))),
            HELLO_REQUIRED
        );
        let r = call(&mut c, &api, "hello", json!({"protocol": 1, "role": "cli"}));
        assert!(r.error.is_none());
        let r = call(&mut c, &api, "system.info", json!({}));
        assert_eq!(r.id, Some(7));
        let info: SystemInfo = serde_json::from_value(r.result.unwrap()).unwrap();
        assert_eq!(info, sample());
    }

    #[test]
    fn version_mismatch_is_explained() {
        let mut c = Connection::default();
        let r = call(
            &mut c,
            &Fake(Ok(sample())),
            "hello",
            json!({"protocol": 2, "role": "ui"}),
        );
        assert_eq!(code(&r), VERSION_MISMATCH);
        assert!(r.error.unwrap().message.contains("protocol 2"));
    }

    #[test]
    fn bad_input_gets_json_rpc_errors() {
        let api = Fake(Ok(sample()));
        let mut c = Connection::default();
        assert_eq!(code(&c.handle(b"not json", &api)), PARSE_ERROR);
        assert_eq!(
            code(&call(&mut c, &api, "hello", json!({"role": "cli"}))),
            INVALID_PARAMS
        );
        call(&mut c, &api, "hello", json!({"protocol": 1, "role": "cli"}));
        assert_eq!(
            code(&call(&mut c, &api, "nope", json!(null))),
            METHOD_NOT_FOUND
        );
        let wrong = br#"{"jsonrpc":"1.0","id":1,"method":"hello"}"#;
        assert_eq!(code(&c.handle(wrong, &api)), INVALID_REQUEST);
    }

    #[test]
    fn os_failure_becomes_internal_error_with_api_name() {
        let api = Fake(Err(OsError {
            api: "GetSystemPowerStatus",
            code: 5,
        }));
        let mut c = Connection::default();
        call(&mut c, &api, "hello", json!({"protocol": 1, "role": "cli"}));
        let r = call(&mut c, &api, "system.info", json!({}));
        assert_eq!(code(&r), INTERNAL_ERROR);
        assert!(r.error.unwrap().message.contains("GetSystemPowerStatus"));
    }
}
