import QtQuick
import QtQuick.Controls
import QtQuick.Layouts
import FinderMaxxing

// The license on this computer: enter a key, see who it's for, free the seat.
Dialog {
    id: dialog

    required property LicenseController license
    required property UpdaterController updater

    parent: Overlay.overlay
    anchors.centerIn: parent
    width: Math.min(480, parent.width - 32)
    modal: true
    title: qsTr("FinderMaxxing %1 %2").arg(updater.version).arg(license.edition)
    standardButtons: Dialog.Close
    onAboutToShow: {
        key.text = ""
        license.refresh()
    }

    ColumnLayout {
        anchors.fill: parent
        spacing: 10

        Label {
            Layout.fillWidth: true
            wrapMode: Text.Wrap
            text: dialog.license.statusText
        }
        Label {
            Layout.fillWidth: true
            wrapMode: Text.Wrap
            visible: !dialog.license.pro
            text: qsTr("Pro adds cloud storage, S3 and servers, AI search and organizing, archives, checksums, duplicates, disk usage, image tools and the terminal.")
            opacity: 0.8
        }
        RowLayout {
            visible: !dialog.license.activated || !dialog.license.pro
            TextField {
                id: key
                Layout.fillWidth: true
                placeholderText: qsTr("License key")
                enabled: !dialog.license.busy
                onAccepted: dialog.license.activate(text)
                Accessible.name: qsTr("License key")
            }
            Button {
                text: qsTr("Activate")
                highlighted: true
                enabled: key.text.trim() !== "" && !dialog.license.busy
                onClicked: dialog.license.activate(key.text)
            }
        }
        RowLayout {
            Button {
                text: qsTr("Buy Pro…")
                visible: !dialog.license.pro && dialog.license.storeUrl !== ""
                onClicked: Qt.openUrlExternally(dialog.license.storeUrl)
            }
            Button {
                text: qsTr("Deactivate This Computer")
                visible: dialog.license.activated
                enabled: !dialog.license.busy
                onClicked: dialog.license.deactivate()
                ToolTip.visible: hovered
                ToolTip.text: qsTr("Frees this computer's seat so the license can be used on another one.")
            }
        }
        BusyIndicator {
            running: dialog.license.busy
            visible: running
        }
        MenuSeparator { Layout.fillWidth: true }
        RowLayout {
            Button {
                text: qsTr("Check for Updates…")
                visible: dialog.updater.mechanism === UpdaterController.Sparkle
                         || dialog.updater.mechanism === UpdaterController.AppImage
                enabled: !dialog.updater.busy
                onClicked: dialog.updater.checkNow()
            }
            Button {
                text: qsTr("Releases Page…")
                visible: dialog.updater.releasesUrl !== "" && dialog.updater.mechanism !== UpdaterController.Sparkle
                onClicked: Qt.openUrlExternally(dialog.updater.releasesUrl)
            }
            CheckBox {
                text: qsTr("Check automatically")
                visible: dialog.updater.mechanism === UpdaterController.Sparkle
                checked: dialog.updater.automatic
                onToggled: dialog.updater.setAutomaticChecks(checked)
            }
        }
        Label {
            Layout.fillWidth: true
            visible: text !== ""
            wrapMode: Text.Wrap
            text: dialog.updater.note
            opacity: 0.8
        }
        Label {
            Layout.fillWidth: true
            visible: dialog.license.errorMessage !== ""
            wrapMode: Text.Wrap
            text: dialog.license.errorMessage
            Accessible.role: Accessible.AlertMessage
        }
    }
}
