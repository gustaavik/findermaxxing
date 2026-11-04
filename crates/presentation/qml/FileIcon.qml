import QtQuick

// Placeholder vector icons. Native per-file icons (NSWorkspace / freedesktop
// icon themes) arrive with previews in M3 through an image provider.
Item {
    id: icon

    required property string kind

    readonly property color folderColor: "#5ea4e8"
    readonly property color fileColor: palette.mid
    readonly property color brokenColor: "#d9534f"
    readonly property real tabHeightRatio: 0.25
    readonly property real tabWidthRatio: 0.45

    Rectangle {
        visible: icon.kind === "folder"
        x: 0
        y: 0
        width: parent.width * icon.tabWidthRatio
        height: parent.height * icon.tabHeightRatio * 2
        radius: 2
        color: icon.folderColor
    }
    Rectangle {
        visible: icon.kind === "folder"
        y: parent.height * icon.tabHeightRatio
        width: parent.width
        height: parent.height * (1 - icon.tabHeightRatio)
        radius: 2
        color: icon.folderColor
    }
    Rectangle {
        visible: icon.kind !== "folder"
        x: parent.width * 0.15
        width: parent.width * 0.7
        height: parent.height
        radius: 2
        color: "transparent"
        border.width: 1
        border.color: icon.kind === "broken-link" ? icon.brokenColor : icon.fileColor

        Text {
            anchors.centerIn: parent
            visible: icon.kind === "link" || icon.kind === "broken-link"
            text: "↗"
            font.pixelSize: parent.height * 0.6
            color: parent.border.color
        }
    }
}
