use std::rc::Rc;
use std::time;

use wasm_bindgen::prelude::*;
use wasm_bindgen::JsValue;
use web_sys::{ErrorEvent, MessageEvent, WebSocket};
use gloo_timers::future::TimeoutFuture;
use futures::{FutureExt, channel::mpsc, StreamExt, select};
use engineio3::Payload;

#[wasm_bindgen]
extern "C" {
    #[wasm_bindgen(js_namespace = JSON, catch)]
    fn parse(s:js_sys::JsString) -> Result<JsValue,JsValue>;
}

pub fn parse_msg(s:&js_sys::JsString) -> Result<JsValue,JsValue> {
    let ss = s.slice(1,s.length() - 1);
    parse(ss)
}

// ==========================
#[wasm_bindgen(js_name=EngineError)]
#[derive(Copy, Clone, Debug)]
pub enum JSError {
    TransportError
}

#[wasm_bindgen]
pub struct Engine {
    transport: Rc<WebSocket>,
}

#[wasm_bindgen]
impl Engine {
    pub fn close(self) {
        todo!()
    }

    pub fn send(&self, msg:&js_sys::JsString) {
        self.transport.send_with_array_buffer_view(msg)
    }
}

// TODO: we should make better types for Function + return error 
#[wasm_bindgen]
pub fn create_ws_engine(url:&str, on_msg:js_sys::Function) -> Result<Engine, JsValue> {
    let ws = WebSocket::new(url)?;
    ws.set_binary_type(web_sys::BinaryType::Arraybuffer);

    let (mut ingress_tx, ingress_rx) = mpsc::channel(64);
    let (mut egress_tx, egress_rx) = mpsc::channel(64);
    let f = Closure::wrap(Box::new(move |v:MessageEvent| {
        let _ = ingress_tx.try_send(v);
    }) as Box<dyn FnMut(MessageEvent)>);
    
    ws.set_onmessage(Some(f.as_ref().unchecked_ref()));
    // TODO: Better please
    f.forget();

    let ws_shared = Rc::new(ws);

    let e = Engine { transport: ws_shared.clone() };
    wasm_bindgen_futures::spawn_local(async move {
        let ee = engineio3::Engine { };
        ws_poll(ee, ingress_rx, egress_rx, ws_shared.as_ref(), on_msg).await;
    });
    Ok(e)
}

// MAIN Event Loop
async fn ws_poll(
    mut e:engineio3::Engine,
    mut ingress_rx:mpsc::Receiver<MessageEvent>, 
    mut egress_rx:mpsc::Receiver<MessageEvent>, 
    ws: &WebSocket,
    callback:js_sys::Function
    )
{
    loop {
        let now = time::Instant::now();
        while let Some(p) = e.poll(now) {
            let data = p.into_bytes();
            let err = match ws.ready_state() {
                web_sys::WebSocket::OPEN => {
                    ws.send_with_u8_array(data.as_slice()).map_err(|_| JSError::TransportError)
                },
                _ => Err(JSError::TransportError)
            };

            if let Err(_e) = err {
                // Close down transport 
                break 
            }

        }
        let Some(t) = e.next_timeout() else { 
            break
        };

        let mut timeout = TimeoutFuture::new(t.as_millis() as u32).fuse();
        let mut next = ingress_rx.next().fuse();

        // TODO: move sending here via chan so we can fully own ws
        select! {
            v = next => {
                if v.is_none() { break } 

                // TODO: what about binary
                let Ok(txt) = v.unwrap().data().dyn_into::<js_sys::JsString>() else { continue };
                let Ok(p) = Payload::from_iter16(&mut txt.iter()) else { continue };

                // We feed engine to update state 
                // state being + socket connection + business logic 
                let Ok(_) = e.handle_input(&p) else {
                    // protocol broken! assume worse and close down engine
                    break
                };

                // we return any msg payloads to listeners
                if let Payload::Msg(_t) = p {
                    if let Ok(data) = parse(txt) {
                        let _ = callback.call1(&data, &data);
                    }
                };
            }
            _ = timeout => {}
        };
    }
}
