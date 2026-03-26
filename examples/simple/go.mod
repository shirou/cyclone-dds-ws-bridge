module github.com/shirou/cyclone-dds-ws-bridge/examples/simple

go 1.26

require (
	github.com/shirou/cyclone-dds-ws-bridge/ddswsclient v0.0.0
	github.com/shirou/go-dds-idlgen v0.0.0
)

require github.com/gorilla/websocket v1.5.3 // indirect

replace (
	github.com/shirou/cyclone-dds-ws-bridge/ddswsclient => ../../ddswsclient
	github.com/shirou/go-dds-idlgen => ../../../go-dds-idlgen
)
