import QtQuick
import QtQuick.Controls
import QtQuick.Layouts
import FinderMaxxing

// Checksums of one file, and a check against a published digest.
Dialog {
    id: dialog

    required property ServicesHolder services

    function openFor(location) {
        expected.text = ""
        controller.start(location)
        open()
    }

    parent: Overlay.overlay
    anchors.centerIn: parent
    width: Math.min(640, parent.width - 32)
    modal: true
    title: qsTr("Checksums of “%1”").arg(controller.fileName)
    standardButtons: Dialog.Close
    onClosed: controller.cancel()

    ChecksumController {
        id: controller
        services: dialog.services
    }

    ColumnLayout {
        anchors.fill: parent
        spacing: 8

        ProgressBar {
            Layout.fillWidth: true
            visible: controller.running
            value: controller.progress
            Accessible.name: qsTr("Reading the file")
        }

        GridLayout {
            Layout.fillWidth: true
            columns: 3
            columnSpacing: 8

            Repeater {
                model: [
                    { name: "MD5", value: controller.md5 },
                    { name: "SHA-1", value: controller.sha1 },
                    { name: "SHA-256", value: controller.sha256 },
                    { name: "BLAKE3", value: controller.blake3 }
                ]
                delegate: Item {
                    required property var modelData
                    Layout.columnSpan: 3
                    Layout.fillWidth: true
                    implicitHeight: digestRow.implicitHeight
                    RowLayout {
                        id: digestRow
                        anchors.fill: parent
                        Label {
                            text: modelData.name
                            Layout.preferredWidth: 70
                        }
                        TextField {
                            Layout.fillWidth: true
                            readOnly: true
                            selectByMouse: true
                            font.family: "monospace"
                            text: modelData.value
                            placeholderText: controller.running ? qsTr("Calculating…") : ""
                            Accessible.name: qsTr("%1 checksum").arg(modelData.name)
                        }
                        Button {
                            text: qsTr("Copy")
                            enabled: modelData.value !== ""
                            onClicked: controller.copyText(modelData.value)
                        }
                    }
                }
            }
        }

        Label {
            Layout.fillWidth: true
            visible: controller.published !== ""
            wrapMode: Text.Wrap
            text: qsTr("A checksum list next to the file names this one.")
        }

        RowLayout {
            Layout.fillWidth: true
            Label {
                text: qsTr("Compare with")
            }
            TextField {
                id: expected
                Layout.fillWidth: true
                placeholderText: qsTr("Paste a checksum from the download page")
                font.family: "monospace"
                enabled: !controller.running && controller.md5 !== ""
                onTextChanged: controller.check(text)
                Accessible.name: qsTr("Published checksum")
            }
        }

        Label {
            Layout.fillWidth: true
            wrapMode: Text.Wrap
            visible: text !== ""
            text: controller.verdict
            font.bold: true
            color: controller.outcome === ChecksumController.Matches ? "#2e7d32"
                   : controller.outcome === ChecksumController.Differs ? "#c62828" : palette.text
            Accessible.role: Accessible.AlertMessage
            Accessible.name: text
        }

        Label {
            Layout.fillWidth: true
            visible: controller.errorMessage !== ""
            wrapMode: Text.Wrap
            text: controller.errorMessage
        }
    }
}
