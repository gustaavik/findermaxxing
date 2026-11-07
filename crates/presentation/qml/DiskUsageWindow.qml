import QtQuick
import QtQuick.Controls
import QtQuick.Layouts
import FinderMaxxing

// Disk Usage: a treemap of one folder level at a time. Click a folder to go
// in, Up to come back; double-click a file to open it. Layout is computed
// in Rust; this only places the rectangles.
ApplicationWindow {
    id: window

    required property ServicesHolder services
    required property OperationsController operations
    property var tiles: []

    readonly property int tileGap: 2
    readonly property int labelMinimumWidth: 60
    readonly property int labelMinimumHeight: 30
    readonly property bool dark: Qt.styleHints.colorScheme === Qt.ColorScheme.Dark

    function openFor(folder) {
        show()
        raise()
        requestActivate()
        controller.measure(folder)
    }

    function relayout() {
        tiles = JSON.parse(controller.tiles(map.width, map.height))
    }

    // Folders in blues and greens, files in warm tones, by position.
    function tileColor(kind, index) {
        const folderHues = [0.58, 0.52, 0.62, 0.46, 0.66]
        const fileHues = [0.08, 0.12, 0.02, 0.15]
        if (kind === "unreadable") {
            return Qt.hsla(0, 0, dark ? 0.25 : 0.8, 1)
        }
        if (kind === "smaller") {
            return Qt.hsla(0, 0, dark ? 0.32 : 0.72, 1)
        }
        const hues = kind === "folder" ? folderHues : fileHues
        return Qt.hsla(hues[index % hues.length], 0.45, dark ? 0.38 : 0.62, 1)
    }

    width: 900
    height: 620
    title: qsTr("Disk Usage")

    DiskUsageController {
        id: controller
        services: window.services
        onChanged: window.relayout()
    }

    onClosing: controller.cancel()

    ColumnLayout {
        anchors.fill: parent
        anchors.margins: 12
        spacing: 8

        RowLayout {
            Layout.fillWidth: true
            Button {
                text: qsTr("Up")
                enabled: controller.canGoUp
                onClicked: controller.up()
            }
            ColumnLayout {
                Layout.fillWidth: true
                spacing: 0
                Label {
                    text: controller.focusTitle
                    font.bold: true
                }
                Label {
                    Layout.fillWidth: true
                    elide: Text.ElideMiddle
                    text: controller.running ? controller.status : controller.focusSummary
                    opacity: 0.7
                }
            }
            BusyIndicator {
                running: controller.running
                visible: running
                implicitHeight: 24
                implicitWidth: 24
            }
            Button {
                text: controller.running ? qsTr("Stop") : qsTr("Measure Again")
                enabled: controller.running || controller.focusLocation !== ""
                onClicked: controller.running ? controller.cancel() : controller.measure(controller.focusLocation)
            }
        }

        Label {
            Layout.fillWidth: true
            visible: controller.errorMessage !== ""
            wrapMode: Text.Wrap
            text: controller.errorMessage
        }

        Item {
            id: map
            Layout.fillWidth: true
            Layout.fillHeight: true
            clip: true
            onWidthChanged: relayoutTimer.restart()
            onHeightChanged: relayoutTimer.restart()

            Timer {
                id: relayoutTimer
                interval: 50
                onTriggered: window.relayout()
            }

            Repeater {
                model: window.tiles
                delegate: Rectangle {
                    id: tile
                    required property var modelData
                    required property int index
                    x: modelData.x + window.tileGap / 2
                    y: modelData.y + window.tileGap / 2
                    width: Math.max(0, modelData.w - window.tileGap)
                    height: Math.max(0, modelData.h - window.tileGap)
                    radius: 3
                    color: hover.hovered ? Qt.lighter(window.tileColor(modelData.kind, index), 1.15)
                                         : window.tileColor(modelData.kind, index)
                    Accessible.role: Accessible.Button
                    Accessible.name: qsTr("%1, %2").arg(modelData.name).arg(modelData.size)

                    Column {
                        anchors.fill: parent
                        anchors.margins: 4
                        visible: tile.width > window.labelMinimumWidth && tile.height > window.labelMinimumHeight
                        Label {
                            width: parent.width
                            text: tile.modelData.name
                            elide: Text.ElideRight
                            font.bold: tile.modelData.kind === "folder"
                        }
                        Label {
                            width: parent.width
                            text: tile.modelData.size + " · " + Math.round(tile.modelData.share * 100) + "%"
                            elide: Text.ElideRight
                            opacity: 0.8
                        }
                    }

                    HoverHandler {
                        id: hover
                    }
                    ToolTip.visible: hover.hovered
                    ToolTip.delay: 400
                    ToolTip.text: modelData.name + " — " + modelData.size
                    TapHandler {
                        onTapped: if (tile.modelData.kind === "folder") controller.open(tile.modelData.location)
                        onDoubleTapped: if (tile.modelData.kind === "file") window.operations.open(tile.modelData.location)
                    }
                }
            }
        }
    }
}
