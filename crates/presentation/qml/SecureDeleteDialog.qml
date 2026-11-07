import QtQuick
import QtQuick.Controls
import QtQuick.Layouts
import FinderMaxxing

// Confirms a secure delete and says plainly what it can and can't promise.
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
    width: Math.min(480, parent.width - 32)
    modal: true
    title: locations.length === 1 ? qsTr("Securely delete 1 item?") : qsTr("Securely delete %1 items?").arg(locations.length)

    ColumnLayout {
        anchors.fill: parent
        spacing: 10
        Label {
            Layout.fillWidth: true
            wrapMode: Text.Wrap
            text: qsTr("The files are overwritten with random data, then deleted. This can't be undone.")
        }
        Label {
            Layout.fillWidth: true
            wrapMode: Text.Wrap
            font.pixelSize: Qt.application.font.pixelSize * 0.9
            text: qsTr("On SSDs and on APFS or Btrfs, the drive or file system may keep earlier copies of the data where nothing can overwrite them. Disk encryption (FileVault, LUKS) is what protects deleted files there.")
        }
    }

    footer: DialogButtonBox {
        Button {
            text: qsTr("Cancel")
            DialogButtonBox.buttonRole: DialogButtonBox.RejectRole
        }
        Button {
            text: qsTr("Securely Delete")
            DialogButtonBox.buttonRole: DialogButtonBox.AcceptRole
        }
    }
    onAccepted: operations.secureDelete(locations)
}
