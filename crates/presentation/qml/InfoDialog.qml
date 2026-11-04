import QtQuick
import QtQuick.Controls
import QtQuick.Layouts
import FinderMaxxing

// "Get Info" for items on S3: content type and metadata (editable), facts,
// versions, and an expiring share link.
Dialog {
    id: dialog

    required property ServicesHolder services

    property string itemName: ""

    readonly property int preferredWidth: 560
    readonly property int maxLinkHours: 168

    function openFor(location, name) {
        itemName = name
        details.load(location)
        open()
    }

    parent: Overlay.overlay
    anchors.centerIn: parent
    width: Math.min(preferredWidth, parent.width - 32)
    modal: true
    title: qsTr("Info: %1").arg(itemName)

    ItemDetailsController {
        id: details
        services: dialog.services
        onSaved: dialog.close()
    }

    ColumnLayout {
        anchors.fill: parent
        spacing: 8

        BusyIndicator {
            Layout.alignment: Qt.AlignHCenter
            running: details.busy
            visible: running
        }
        Label {
            Layout.fillWidth: true
            visible: details.errorMessage !== ""
            text: details.errorMessage
            wrapMode: Text.Wrap
            color: "#d9534f"
        }
        GridLayout {
            Layout.fillWidth: true
            columns: 2
            Label { text: qsTr("Content type:") }
            TextField {
                id: contentType
                Layout.fillWidth: true
                text: details.contentType
                Accessible.name: qsTr("Content type")
            }
            Label { text: qsTr("Metadata:"); Layout.alignment: Qt.AlignTop }
            TextArea {
                id: metadata
                Layout.fillWidth: true
                Layout.preferredHeight: 80
                text: details.metadataText
                placeholderText: qsTr("One “key: value” per line")
                Accessible.name: qsTr("Custom metadata")
            }
        }
        Label {
            Layout.fillWidth: true
            visible: details.facts.length > 0
            text: details.facts.join("\n")
            color: palette.placeholderText
            wrapMode: Text.Wrap
        }
        Label {
            visible: details.versions.length > 0
            text: qsTr("Versions")
            font.bold: true
        }
        ListView {
            Layout.fillWidth: true
            Layout.preferredHeight: Math.min(contentHeight, 120)
            visible: details.versions.length > 0
            clip: true
            model: details.versions
            delegate: Label {
                required property string modelData
                text: modelData
                font.family: Qt.platform.os === "osx" ? "Menlo" : "monospace"
            }
        }
        RowLayout {
            Layout.fillWidth: true
            Label { text: qsTr("Share link valid for") }
            SpinBox {
                id: hours
                from: 1
                to: dialog.maxLinkHours
                value: 24
                editable: true
            }
            Label { text: qsTr("hours") }
            Item { Layout.fillWidth: true }
            Button {
                text: qsTr("Create Link")
                enabled: !details.busy
                onClicked: details.makeShareLink(hours.value)
            }
        }
        RowLayout {
            Layout.fillWidth: true
            visible: details.shareLink !== ""
            TextField {
                Layout.fillWidth: true
                readOnly: true
                text: details.shareLink
                Accessible.name: qsTr("Share link")
            }
            Button {
                text: qsTr("Copy")
                onClicked: details.copyShareLink()
            }
        }
    }

    footer: DialogButtonBox {
        Button {
            text: qsTr("Close")
            DialogButtonBox.buttonRole: DialogButtonBox.RejectRole
        }
        Button {
            text: qsTr("Save")
            highlighted: true
            enabled: !details.busy && details.errorMessage === ""
            onClicked: details.save(contentType.text, metadata.text)
        }
    }
}
