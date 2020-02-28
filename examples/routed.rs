extern crate civet;
extern crate conduit;
extern crate route_recognizer;

use std::sync::mpsc::channel;

use civet::{Config, Server};
use conduit::{box_error, HandlerResult, HttpResult, RequestExt, Response};
use conduit::{static_to_body, vec_to_body};
use route_recognizer::{Params, Router};

struct MyServer {
    router: Router<fn(&mut dyn RequestExt, &Params) -> HttpResult>,
}

impl conduit::Handler for MyServer {
    fn call(&self, req: &mut dyn RequestExt) -> HandlerResult {
        let hit = match self.router.recognize(req.path()) {
            Ok(m) => m,
            Err(e) => panic!("{}", e),
        };
        (*hit.handler)(req, &hit.params).map_err(box_error)
    }
}

fn main() {
    let mut server = MyServer {
        router: Router::new(),
    };
    server.router.add("/:id", id);
    server.router.add("/", root);
    let _a = Server::start(Config::new(), server);
    let (_tx, rx) = channel::<()>();
    rx.recv().unwrap();
}

fn root(_req: &mut dyn RequestExt, _params: &Params) -> HttpResult {
    let bytes = b"you found the root!\n";
    Response::builder().body(static_to_body(bytes))
}

fn id(_req: &mut dyn RequestExt, params: &Params) -> HttpResult {
    let string = format!("you found the id {}!\n", params["id"]);
    Response::builder().body(vec_to_body(string.into_bytes()))
}
