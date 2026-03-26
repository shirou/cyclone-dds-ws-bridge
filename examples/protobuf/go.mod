module github.com/shirou/cyclone-dds-ws-bridge/examples/protobuf

go 1.26

require (
	github.com/shirou/cyclone-dds-ws-bridge/ddswsclient v0.0.0
	google.golang.org/protobuf v1.36.6
)

require github.com/gorilla/websocket v1.5.3 // indirect

replace github.com/shirou/cyclone-dds-ws-bridge/ddswsclient => ../../ddswsclient
