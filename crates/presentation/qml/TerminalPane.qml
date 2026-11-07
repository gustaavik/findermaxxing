import QtQuick
import QtQuick.Controls
import QtQuick.Layouts
import FinderMaxxing

// The terminal pane under the folder views. The shell starts in the folder
// being viewed and follows it while it waits at its prompt.
Pane {
    id: pane

    required property ServicesHolder services
    readonly property alias controller: controller
    // The folder of the current tab (a location id).
    property string folder: ""
    signal closeRequested()

    function start() {
        if (!controller.running) {
            controller.open(folder, view.columns, view.rows)
        }
        view.forceActiveFocus()
    }

    padding: 0
    onFolderChanged: if (controller.running) controller.follow(folder)

    TerminalController {
        id: controller
        services: pane.services
        view: view
        dark: Qt.styleHints.colorScheme === Qt.ColorScheme.Dark
    }

    ColumnLayout {
        anchors.fill: parent
        spacing: 0

        RowLayout {
            Layout.fillWidth: true
            Layout.leftMargin: 8
            Label {
                Layout.fillWidth: true
                text: controller.running ? (controller.title || qsTr("Terminal")) : qsTr("Terminal — the shell has exited")
                elide: Text.ElideMiddle
                opacity: 0.8
            }
            ToolButton {
                text: qsTr("Restart")
                visible: !controller.running
                onClicked: pane.start()
            }
            ToolButton {
                text: "✕"
                onClicked: {
                    controller.close()
                    pane.closeRequested()
                }
                Accessible.name: qsTr("Close the terminal")
            }
        }

        TerminalView {
            id: view
            Layout.fillWidth: true
            Layout.fillHeight: true
            focus: true
            Accessible.role: Accessible.EditableText
            Accessible.name: qsTr("Terminal")

            onGridSizeChanged: controller.resize(columns, rows)
            onKeyTyped: (key, modifiers, text) => controller.key(key, modifiers, text)
            onTextTyped: text => controller.typeText(text)
            onScrolled: lines => controller.scroll(lines)
            onSelecting: (fromRow, fromColumn, toRow, toColumn) => controller.select(fromRow, fromColumn, toRow, toColumn)
            onSelectionCleared: controller.clearSelection()
            onCopyRequested: controller.copy()
            onPasteRequested: controller.paste()
        }
    }
}
