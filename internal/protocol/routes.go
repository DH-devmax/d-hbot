package protocol

const DefaultBaseURL = "http://127.0.0.1:51234"

const (
	RoutePing              = "/ping"
	RouteSendMessage       = "/v1/plugins/send-msg"
	RouteListenMessages    = "/v1/plugins/listen-msg"
	RoutePollMessages      = "/v1/plugins/poll-msg"
	RoutePeekMessages      = "/v1/plugins/peek-msg"
	RouteAckMessages       = "/v1/plugins/ack-msg"
	RouteSessionInfo       = "/v1/plugins/session-info"
	RouteNIMTeamMembers    = "/v1/plugins/nim-team-members"
	RouteNIMUpdateNick     = "/v1/plugins/nim-update-nick"
	RouteGroupList         = "/v1/group/get-group-list"
	RouteGroupMembers      = "/v1/group/get-group-members"
	RouteSetGroupMute      = "/v1/group/set-group-mute"
	RouteSetMemberMute     = "/v1/group/set-member-mute"
	RouteCancelMemberMute  = "/v1/group/member-mute-cancel"
	RouteSetMemberNickname = "/v1/group/set-member-nickname"
	RouteRemoveGroupMember = "/v1/group/remove-group-member"
	RouteRollbackMessage   = "/v1/group/message-rollback"
)
