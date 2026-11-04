import QtQuick
import QtQuick.Controls
import QtQuick.Dialogs
import QtQuick.Layouts
import FinderMaxxing

// AI search settings: turning it on (after showing what will be downloaded,
// from where), what it covers, photo search, pausing, and turning it off.
Dialog {
    id: dialog

    required property AiSearchController controller

    readonly property int preferredWidth: 560

    parent: Overlay.overlay
    anchors.centerIn: parent
    width: Math.min(preferredWidth, parent.width - 32)
    modal: true
    title: qsTr("AI Search")
    standardButtons: Dialog.Close
    onAboutToShow: controller.refresh()

    ColumnLayout {
        anchors.fill: parent
        spacing: 12

        // Off: what turning it on means.
        ColumnLayout {
            Layout.fillWidth: true
            visible: !dialog.controller.enabled
            Label {
                Layout.fillWidth: true
                wrapMode: Text.Wrap
                text: qsTr("Search your files by what they say, in your own words and in many languages: “invoices from last spring”, “kvartalsbudget”, “notes about the move”.")
            }
            Label {
                Layout.fillWidth: true
                wrapMode: Text.Wrap
                text: qsTr("This downloads a language model once: %1. After that everything runs on this computer: your files and searches never leave it.").arg(dialog.controller.textOffer)
                color: palette.placeholderText
            }
            Button {
                text: qsTr("Download and Turn On")
                highlighted: true
                visible: !dialog.controller.installing
                onClicked: dialog.controller.enableText()
            }
        }

        // A download in progress (text or images).
        ColumnLayout {
            Layout.fillWidth: true
            visible: dialog.controller.installing
            Label { text: dialog.controller.installLabel }
            RowLayout {
                Layout.fillWidth: true
                ProgressBar {
                    Layout.fillWidth: true
                    value: dialog.controller.installProgress
                    Accessible.name: qsTr("Model download")
                }
                Button {
                    text: qsTr("Cancel")
                    onClicked: dialog.controller.cancelInstall()
                }
            }
        }

        // On: status, scope, photos, off.
        ColumnLayout {
            Layout.fillWidth: true
            visible: dialog.controller.enabled
            spacing: 10

            RowLayout {
                Layout.fillWidth: true
                Label {
                    Layout.fillWidth: true
                    text: dialog.controller.statusText
                    elide: Text.ElideMiddle
                    Accessible.role: Accessible.StatusBar
                }
                Button {
                    text: dialog.controller.paused ? qsTr("Resume") : qsTr("Pause")
                    onClicked: dialog.controller.setIndexingPaused(!dialog.controller.paused)
                }
            }
            ProgressBar {
                Layout.fillWidth: true
                visible: dialog.controller.working
                indeterminate: dialog.controller.progress < 0
                value: Math.max(0, dialog.controller.progress)
            }

            Label { text: qsTr("Searched folders"); font.bold: true }
            Repeater {
                model: dialog.controller.folders
                delegate: RowLayout {
                    required property int index
                    required property string modelData
                    Layout.fillWidth: true
                    Label { Layout.fillWidth: true; text: modelData; elide: Text.ElideMiddle }
                    ToolButton {
                        text: "✕"
                        onClicked: dialog.controller.forgetFolder(dialog.controller.folderIds[index])
                        Accessible.name: qsTr("Stop searching %1").arg(modelData)
                    }
                }
            }
            Label {
                visible: dialog.controller.excluded.length > 0
                text: qsTr("Left out")
                font.bold: true
            }
            Repeater {
                model: dialog.controller.excluded
                delegate: RowLayout {
                    required property int index
                    required property string modelData
                    Layout.fillWidth: true
                    Label { Layout.fillWidth: true; text: modelData; elide: Text.ElideMiddle; color: palette.placeholderText }
                    ToolButton {
                        text: "✕"
                        onClicked: dialog.controller.forgetFolder(dialog.controller.excludedIds[index])
                        Accessible.name: qsTr("Stop leaving out %1").arg(modelData)
                    }
                }
            }
            RowLayout {
                Button {
                    text: qsTr("Add Folder…")
                    onClicked: { folderPicker.excluding = false; folderPicker.open() }
                }
                Button {
                    text: qsTr("Leave Out a Folder…")
                    onClicked: { folderPicker.excluding = true; folderPicker.open() }
                }
            }
            Label {
                Layout.fillWidth: true
                wrapMode: Text.Wrap
                font.pointSize: Qt.application.font.pointSize * 0.85
                color: palette.placeholderText
                text: qsTr("Folders on servers and cloud drives can be added too; their files are downloaded to be read.")
            }

            MenuSeparator { Layout.fillWidth: true }

            CheckBox {
                text: qsTr("Search photos by what they show")
                checked: dialog.controller.imagesEnabled
                enabled: !dialog.controller.installing
                onClicked: {
                    if (checked) {
                        checked = false
                        imagesConsent.open()
                    } else {
                        dialog.controller.turnOffImages(false)
                    }
                }
            }

            MenuSeparator { Layout.fillWidth: true }

            RowLayout {
                Button {
                    text: qsTr("Turn Off…")
                    onClicked: turnOff.open()
                }
                Item { Layout.fillWidth: true }
                Button {
                    text: qsTr("Look for Changes Now")
                    onClicked: dialog.controller.rescan()
                }
            }
        }

        Label {
            Layout.fillWidth: true
            visible: dialog.controller.errorMessage !== ""
            text: dialog.controller.errorMessage
            wrapMode: Text.Wrap
            color: "#d9534f"
            Accessible.role: Accessible.AlertMessage
        }
    }

    FolderDialog {
        id: folderPicker
        property bool excluding: false
        title: excluding ? qsTr("Leave out a folder") : qsTr("Search a folder")
        onAccepted: {
            const location = selectedFolder.toString()
            if (excluding) {
                dialog.controller.excludeFolder(location)
            } else {
                dialog.controller.includeFolder(location)
            }
        }
    }

    Dialog {
        id: imagesConsent
        parent: Overlay.overlay
        anchors.centerIn: parent
        width: Math.min(480, parent.width - 32)
        modal: true
        title: qsTr("Search photos by what they show?")
        standardButtons: Dialog.Cancel | Dialog.Ok
        onAccepted: dialog.controller.enableImages()
        ColumnLayout {
            anchors.fill: parent
            Label {
                Layout.fillWidth: true
                wrapMode: Text.Wrap
                text: qsTr("This downloads image models once: %1. Photos are looked at on this computer only.").arg(dialog.controller.imageOffer)
            }
        }
    }

    Dialog {
        id: turnOff
        parent: Overlay.overlay
        anchors.centerIn: parent
        width: Math.min(480, parent.width - 32)
        modal: true
        title: qsTr("Turn off AI search?")
        standardButtons: Dialog.Cancel | Dialog.Ok
        onAccepted: dialog.controller.turnOff(forget.checked)
        ColumnLayout {
            anchors.fill: parent
            Label {
                Layout.fillWidth: true
                wrapMode: Text.Wrap
                text: qsTr("Indexing stops. Searching by name keeps working.")
            }
            CheckBox {
                id: forget
                text: qsTr("Also delete the index and the downloaded models")
            }
        }
    }
}
