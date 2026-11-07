import QtQuick
import QtQuick.Controls
import QtQuick.Layouts
import FinderMaxxing

// Picks the archive format; the job itself belongs to OperationsController.
Dialog {
    id: dialog

    required property OperationsController operations
    property var locations: []

    function openFor(items) {
        locations = items
        open()
    }

    parent: Overlay.overlay
    anchors.centerIn: parent
    width: Math.min(400, parent.width - 32)
    modal: true
    title: locations.length === 1 ? qsTr("Compress 1 item") : qsTr("Compress %1 items").arg(locations.length)
    standardButtons: Dialog.Cancel | Dialog.Ok
    onAccepted: operations.compress(locations, format.currentIndex)

    ColumnLayout {
        anchors.fill: parent
        Label {
            text: qsTr("Format")
        }
        ComboBox {
            id: format
            Layout.fillWidth: true
            model: dialog.operations.archiveFormats()
            Accessible.name: qsTr("Archive format")
        }
        Label {
            Layout.fillWidth: true
            wrapMode: Text.Wrap
            font.pixelSize: Qt.application.font.pixelSize * 0.9
            text: qsTr("ZIP opens everywhere. 7-Zip and tar.xz compress best; tar.zst is fastest.")
        }
    }
}
