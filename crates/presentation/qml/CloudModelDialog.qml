import QtQuick
import QtQuick.Controls
import QtQuick.Layouts
import FinderMaxxing

// Bring your own cloud model: provider, model and key (kept in the
// keychain), and an explicit per-folder opt-in that shows exactly what
// would be sent, and to whom, first.
Dialog {
    id: dialog

    required property CloudModelController controller
    property string folder: ""
    property string folderTitle: ""

    parent: Overlay.overlay
    anchors.centerIn: parent
    width: Math.min(620, parent.width - 32)
    height: Math.min(640, parent.height - 32)
    modal: true
    title: qsTr("Cloud Model")
    standardButtons: Dialog.Close
    onAboutToShow: {
        controller.refresh(folder)
        preview.visible = false
    }

    Connections {
        target: dialog.controller
        function onChanged() {
            provider.currentIndex = dialog.controller.provider === CloudModelController.OpenAiCompatible ? 1 : 0
            baseUrl.text = dialog.controller.baseUrl
            model.text = dialog.controller.model
            key.text = ""
        }
    }

    ScrollView {
        anchors.fill: parent
        clip: true
        ColumnLayout {
            width: dialog.availableWidth - 16
            spacing: 8

            Label {
                Layout.fillWidth: true
                wrapMode: Text.Wrap
                text: qsTr("Suggestions are made on this computer. Optionally, your own cloud model can suggest better folder names and file names. It's only asked about folders you allow, one at a time, and you pay its provider directly.")
            }
            GridLayout {
                Layout.fillWidth: true
                columns: 2
                Label { text: qsTr("Service") }
                ComboBox {
                    id: provider
                    Layout.fillWidth: true
                    model: [qsTr("Anthropic (Claude)"), qsTr("OpenAI-compatible (OpenAI, Mistral, Ollama, …)")]
                    onActivated: index => {
                        const chosen = index === 1 ? CloudModelController.OpenAiCompatible : CloudModelController.Anthropic
                        baseUrl.text = dialog.controller.defaultBaseUrl(chosen)
                        model.text = dialog.controller.defaultModel(chosen)
                    }
                }
                Label { text: qsTr("Address") }
                TextField {
                    id: baseUrl
                    Layout.fillWidth: true
                    Accessible.name: qsTr("API address")
                }
                Label { text: qsTr("Model") }
                TextField {
                    id: model
                    Layout.fillWidth: true
                    placeholderText: qsTr("e.g. gpt-5 or llama3.3")
                    Accessible.name: qsTr("Model")
                }
                Label { text: qsTr("API key") }
                TextField {
                    id: key
                    Layout.fillWidth: true
                    echoMode: TextInput.Password
                    placeholderText: dialog.controller.hasKey ? qsTr("Saved in the keychain; type to replace")
                                                              : qsTr("Not needed for a local server")
                    Accessible.name: qsTr("API key")
                }
            }
            RowLayout {
                Button {
                    text: qsTr("Save")
                    highlighted: true
                    enabled: !dialog.controller.busy
                    onClicked: dialog.controller.save(provider.currentIndex === 1 ? CloudModelController.OpenAiCompatible
                                                                                  : CloudModelController.Anthropic,
                                                      baseUrl.text, model.text, key.text)
                }
                Button {
                    text: qsTr("Remove Cloud Model")
                    visible: dialog.controller.configured || dialog.controller.hasKey
                    onClicked: dialog.controller.forget()
                }
            }

            MenuSeparator { Layout.fillWidth: true }

            Label {
                Layout.fillWidth: true
                font.bold: true
                text: qsTr("“%1”").arg(dialog.folderTitle)
            }
            Label {
                Layout.fillWidth: true
                wrapMode: Text.Wrap
                text: dialog.controller.optedIn
                      ? qsTr("Allowed: when you analyze this folder, a summary of it is sent to %1.").arg(dialog.controller.recipient)
                      : qsTr("If you allow it, analyzing this folder sends a summary to %1: the folder's name and, for up to 200 items, their names, kinds, sizes and dates, and the first 400 characters of documents' text. Never whole files, never images, never anything outside this folder.").arg(dialog.controller.recipient || qsTr("the service above"))
            }
            RowLayout {
                Button {
                    text: qsTr("Show What Would Be Sent")
                    enabled: dialog.folder !== ""
                    onClicked: {
                        preview.visible = true
                        dialog.controller.loadPreview(dialog.folder)
                    }
                }
                Button {
                    text: dialog.controller.optedIn ? qsTr("Stop for This Folder") : qsTr("Allow for This Folder")
                    enabled: dialog.controller.configured && dialog.folder !== ""
                    onClicked: dialog.controller.setOptIn(dialog.folder, !dialog.controller.optedIn)
                }
            }
            TextArea {
                id: preview
                Layout.fillWidth: true
                Layout.preferredHeight: 220
                visible: false
                readOnly: true
                wrapMode: TextEdit.Wrap
                font.family: "monospace"
                text: dialog.controller.busy ? qsTr("Reading the folder…") : dialog.controller.previewText
                Accessible.name: qsTr("What would be sent")
            }
            Label {
                Layout.fillWidth: true
                visible: dialog.controller.errorMessage !== ""
                wrapMode: Text.Wrap
                text: dialog.controller.errorMessage
            }
        }
    }
}
