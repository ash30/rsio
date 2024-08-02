use std::ops::Add;

use log::{info, Level};
use web_time::{Duration, Instant};

use engineio3::RawPayload;
use engineio3::{Message, Payload};
use futures::{channel::mpsc, select, FutureExt, StreamExt};
use gloo_timers::future::TimeoutFuture;
use wasm_bindgen::prelude::*;
use wasm_bindgen::JsValue;
use web_sys::{MessageEvent, WebSocket};

#[wasm_bindgen]
extern "C" {}

#[derive(Clone, Copy, PartialEq, PartialOrd, Debug)]
struct JsTime(Instant);

impl Add<std::time::Duration> for JsTime {
    type Output = Self;
    fn add(self, rhs: std::time::Duration) -> Self::Output {
        let d = Duration::from_millis(rhs.as_millis().try_into().unwrap());
        JsTime(self.0 + d)
    }
}

impl JsTime {
    fn now() -> Self {
        JsTime(Instant::now())
    }
}

// ==========================
struct WSTextMessage(js_sys::JsString);

// We want to avoid copying the whole body into wasm
// just to dispatch backout...
// so we cannot JUST pass a byte array to CORE
// instead we pass a handle to the data
// but Sometimes... we do want the bytes FFS
// so we have both options
// TODO: Body should be INTO bytes
impl RawPayload for WSTextMessage {
    type U = JsValue;
    fn body(&self) -> Option<Self::U> {
        if self.0.length() == 1 {
            None
        } else {
            Some(self.0.slice(1, self.0.length()).into())
        }
    }
    fn body_as_bytes(&self) -> Vec<u8> {
        self.0.iter().skip(1).map(|a| a.to_be_bytes()[1]).collect()
    }
    fn prefix(&self) -> u8 {
        self.0
            .iter()
            .next()
            .map(|a| a.to_be_bytes()[1])
            .unwrap_or(99)
    }
}

// ==========================
#[wasm_bindgen(js_name=EngineError)]
#[derive(Copy, Clone, Debug)]
pub enum JSError {
    TransportError,
}

#[wasm_bindgen]
pub struct Engine {
    // TODO: should we support full times of WS?
    tx: mpsc::Sender<engineio3::Message<JsValue>>,
}

#[wasm_bindgen]
impl Engine {
    pub fn close(self) {
        todo!()
    }

    pub fn send(&mut self, msg: js_sys::JsString) -> Result<(), JsValue> {
        // TODO: ERROR please
        let m: Message<JsValue> = Message::Text(msg.into());
        let _ = self.tx.try_send(m);
        Ok(())
    }
}

// TODO: we should make better types for Function + return error
// on msg should be an enum : connection || message
#[wasm_bindgen]
pub async fn create_ws_engine(url: &str, on_msg: js_sys::Function) -> Result<Engine, JsValue> {
    console_log::init_with_level(Level::Debug);

    let (mut ingress_tx, ingress_rx) = mpsc::channel(64);
    let (egress_tx, egress_rx) = mpsc::channel(64);

    let ws = WebSocket::new(url)?;
    ws.set_binary_type(web_sys::BinaryType::Arraybuffer);
    let e = Engine { tx: egress_tx };
    wasm_bindgen_futures::spawn_local(async move {
        let f = Closure::wrap(Box::new(move |v: MessageEvent| {
            let _ = ingress_tx.try_send(v);
        }) as Box<dyn FnMut(MessageEvent)>);
        ws.set_onmessage(Some(f.as_ref().unchecked_ref()));
        let state = engineio3::Engine::<JsValue, JsTime>::new(JsTime::now());
        ws_poll(state, ingress_rx, egress_rx, &ws, on_msg).await;
        // close ws so callback is not called again
        let _ = ws.close();
    });
    Ok(e)
}

// MAIN Event Loop
async fn ws_poll(
    mut e: engineio3::Engine<JsValue, JsTime>,
    mut ingress_rx: mpsc::Receiver<MessageEvent>,
    mut egress_rx: mpsc::Receiver<Message<JsValue>>,
    ws: &WebSocket,
    callback: js_sys::Function,
) {
    info!("loop start");
    loop {
        let now = Instant::now();

        // Send out any pending payloads
        // either messages or protocol specifics
        while let Some(p) = e.poll_output(JsTime::now()) {
            info!("out 1");

            if ws.ready_state() != web_sys::WebSocket::OPEN {
                break;
            }
            let err = match p {
                Payload::Msg(Message::Text(m)) => {
                    // TODO: HACK! yolo
                    let o: js_sys::Object = m.unchecked_into();
                    // TODO:Hpw to prefix...
                    ws.send_with_array_buffer_view(&o)
                        .map_err(|_| JSError::TransportError)
                }
                Payload::Msg(Message::Binary(m)) => {
                    // TODO:
                    Ok(())
                }
                _ => {
                    // For non Message, we own the data so serialise please
                    let data = p.into_bytes();
                    ws.send_with_u8_array(data.as_slice())
                        .map_err(|_| JSError::TransportError)
                }
            };
            // IF any sending fails, close out
            if let Err(_e) = err {
                // Close down transport
                break;
            }
        }

        // receive new input + timeout
        let Some(t) = e.next_deadline() else { break };

        let d = t.0 - Instant::now();
        let mut timeout = TimeoutFuture::new(d.as_millis() as u32).fuse();
        let mut next_in = ingress_rx.next().fuse();
        let mut next_out = egress_rx.next().fuse();

        info!("in 1");
        select! {
            m = next_out => {
                // JS side has dropped sender... lets drop ?
                if m.is_none() { break }
                // Error if engine is closed etc
                let Ok(_) = e.handle_input(&m.unwrap().into(), JsTime::now()) else {
                    // protocol broken! assume worse and close down engine
                    break
                };
            },

            v = next_in => {
                if v.is_none() { break }
                // TODO: what about binary
                let Ok(txt) = v.unwrap().data().dyn_into::<js_sys::JsString>() else { continue };
                //let Ok(p) = Payload::from_iter16(&mut txt.iter()) else { continue };
                info!("INPUT {:?}", txt);
                let r = engineio3::decode(WSTextMessage(txt.trim()));
                let Ok(p) = r else { info!("decode error {:?}",r); continue } ;

                // We feed engine to update state
                // state being + socket connection + business logic
                let Ok(_) = e.handle_input(&p, JsTime::now()) else {
                    // protocol broken! assume worse and close down engine
                    info!("ERROR");
                    break
                };
                info!("engine state{:?}", e);

                match p {
                    Payload::Msg(Message::Text(v)) => {
                        // We pass reference back to JS... but what happens to ownership??
                        // JS will see the real value assumedly ( the real value lives in JS!)
                        // so normal js ref count on object
                        // the handle stays Rust side ... and dies normally ( assumedly ... )
                        info!("bar 1");
                        let _ = callback.call1(&JsValue::NULL, &v);
                        info!("bar 2");
                    }
                    Payload::Msg(Message::Binary(v)) => {
                        info!("bar 3");
                        let _ = callback.call1(&JsValue::NULL,&v);
                        info!("bar 4");
                    }
                    _ => {}
                }
            }
            _ = timeout => {}
        };
    }
}
