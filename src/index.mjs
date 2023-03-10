let url = new URL("/state", window.location.href);
url.protocol = url.protocol.replace("http", "ws");

let ws = new WebSocket(url.href);
ws.onmessage = (ev) => {
	let json = JSON.parse(ev.data);
	console.log(json)

	let el = document.getElementById("controls")

	// Doing nasty things so I don't have to get webpack to work
	let inner = `<div id="activeButtons">`
	for (const videoSource of json.videoSources) {
		if (videoSource.id == json.activeVideoSourceId) {
			inner += `<button class="active" onClick="window.setActiveSource('${videoSource.id}')">${videoSource.name}</button>`
		} else {
			inner += `<button onClick="window.setActiveSource('${videoSource.id}')">${videoSource.name}</button>`
		}
	}
	inner += `</div>`
	inner += `<div id="nextButtons">`
	for (const videoSource of json.videoSources) {
		if (videoSource.id == json.nextVideoSourceId) {
			inner += `<button class="next" onClick="window.setNextSource('${videoSource.id}')">${videoSource.name}</button>`
		} else {
			inner += `<button onClick="window.setNextSource('${videoSource.id}')">${videoSource.name}</button>`
		}
	}
	inner += `</div>`
	el.innerHTML = inner
};

let pc = new RTCPeerConnection({
	iceServers: [
		{
			urls: 'stun:stun.l.google.com:19302'
		}
	]
})
pc.ontrack = function (event) {
	console.log(`${event.track.kind} track added`)
	var el = document.createElement(event.track.kind)
	el.srcObject = event.streams[0]
	el.autoplay = true
	el.controls = true
	el.muted = true // Required for autoplay

	event.track.onmute = function (event) {
		el.parentNode.removeChild(el);
	}

	document.getElementById('remoteVideos').appendChild(el)
}

window.setActiveSource = (id) => {
	ws.send(JSON.stringify({ command: 'setActiveInput', input: id }))
}

window.setNextSource = (id) => {
	ws.send(JSON.stringify({ command: 'setNextInput', input: id }))
}

window.cut = () => {
	ws.send(JSON.stringify({ command: 'doCut' }))
}

window.mix = (val) => {
	let position = val / 100
	if (position < 0.05) {
		position = 0.0
	} else if (position > 0.95) {
		position = 1.0
	}

	ws.send(JSON.stringify({ command: 'doMix', position }))
}

async function doSignaling(pc) {
	let offer = await pc.createOffer();
	pc.setLocalDescription(offer);
	let res = await fetch(
		`http://127.0.0.1:9091/addMedia`,
		{
			method: 'post',
			headers: {
				'Accept': 'application/json, text/plain, */*',
				'Content-Type': 'application/json'
			},
			body: JSON.stringify(offer)
		}
	)
	res = await res.json()
	pc.setRemoteDescription(res)
}

async function addMedia(pc) {
	console.log(`Adding media`)
	pc.addTransceiver('video')
	pc.addTransceiver('audio')

	await doSignaling(pc)
}

// Create a noop DataChannel. By default PeerConnections do not connect
// if they have no media tracks or DataChannels
pc.createDataChannel('noop')
await doSignaling(pc, 'createPeerConnection')
setTimeout(async () => await addMedia(pc), 2000) // Bad! But Browsers like to kill video tags that appear magically on page load...
