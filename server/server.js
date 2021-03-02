const hypercore = require('hypercore')
const Server = require('simple-websocket/server')
const ram = require('random-access-memory')
const fs = require('fs')
const { pipeline } = require('stream')
const http = require('http')
const p = require('path')
const { Server: StaticServer } = require('node-static')
const split = require('split2')

const PORT = 9000

// Init a demo feed.
const feed = hypercore(ram)
feed.ready(() => {
  console.log('key', feed.key.toString('hex'))
  // feed.append(['foo', 'bar'])
  // Append the README, splitted by lines.
  const filename = p.join(__dirname, '..', '..', 'README.md')
  pipeline(
    fs.createReadStream(filename),
    split(),
    feed.createWriteStream(),
    err => {
      if (err) console.error('error importing file', err)
      else console.error('import done, new len %o, bytes %o', feed.length, feed.byteLength)
    }
  )
})

// Statically serve parent dir.
const file = new StaticServer(p.join(__dirname, '..'))
const server = http.createServer(function (request, response) {
  // Return the key as JSON.
  if (request.url === '/key') {
    response.setHeader('Content-Type', 'application/json')
    return response.end(JSON.stringify({ key: feed.key.toString('hex') }))
  }
  // Register the static file server.
  request.addListener('end', function () {
    file.serve(request, response)
  }).resume()
})
server.on('error', err => console.log('server error:', err.message))

// Create a websocket server on top of the http server.
const websocket = new Server({ server })
websocket.on('connection', function (socket) {
  console.log('websocket connection open')
  // Pipe out feed's replication stream into the websocket.
  const stream = feed.replicate(false, { live: true })
  pipeline(stream, socket, stream, err => {
    console.log('replication error', err.message)
  })
  socket.on('close', () => console.log('websocket connection closed'))
  socket.on('error', err => console.log('websocket error:', err.message))
})
websocket.on('error', err => console.log('websocket server error:', err.message))

// Start listening for connections.
server.listen(PORT, () => {
  console.log(`Listening on http://localhost:${PORT}`)
})
