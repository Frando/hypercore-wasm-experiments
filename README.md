# hypercore-protocol in Rust, compiled to WASM

I wrote this some time back and updated it roughly to the current dev branch of [hypercore-protocol-rs](https://github.com/datrs/hypercore-protocol-rs).

*Note: this is very rough still, not optimized, and not cleanly structured :)*


What this does (in Rust compiled to WASM):

- Fetch a key, encoded as hex string, from `/fetch`
- Open a Websocket to localhost:9000
- Open a hypercore-protocol stream on the websocket
- Open a channel for the key that was fetched before
- Load all data blocks, and display them on the page (as a string in a `pre` element)

A Node.js server has the simple demo backend:

- Create a hypercore feed in-memory
- Append the contents of this README file
- Open an HTTP server
- On `/fetch` send the hypercore's key as a hex string
- Serve the static files (index.html, index.js from this dir plus the WASM created through wasm-pack in `/pkg`)
- On other requests, open a websocket connection and pipe it to the replication stream of the hypercore

## How to run

```bash
wasm-pack build --dev --target web
cd server
yarn
node server.js
# open http://localhost:9000
```

If it works, this should display this README in the browser, loaded over hypercore-protocol in Rust in WASM :-)

Check the browser console for some logs. It currently needs quite a while until the content is displayed, I don't know yet why this is. 
