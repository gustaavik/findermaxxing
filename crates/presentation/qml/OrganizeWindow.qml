import QtQuick
import QtQuick.Controls
import QtQuick.Layouts
import FinderMaxxing

// Organize: suggestions for one folder, each accepted or not, with a
// before/after preview of exactly the accepted ones. Nothing changes until
// Apply, and Apply is one Undo step.
ApplicationWindow {
    id: window

    required property ServicesHolder services
    required property OperationsController operations
    property string folder: ""
    property var suggestions: []
    property var preview: []

    readonly property var sections: [
        { key: "cleanup", title: qsTr("Clean up") },
        { key: "folders", title: qsTr("New folders") },
        { key: "renames", title: qsTr("Clearer names") },
        { key: "tags", title: qsTr("Tags") }
    ]

    function openFor(location) {
        folder = location
        show()
        raise()
        requestActivate()
        cloud.refresh(location)
        organize.analyze(location, true)
    }

    function inSection(key) {
        return suggestions.filter(s => s.section === key)
    }

    width: 980
    height: 640
    title: qsTr("Organize “%1”").arg(organize.folderTitle)

    OrganizeController {
        id: organize
        services: window.services
        onSuggestionsJsonChanged: window.suggestions = JSON.parse(suggestionsJson)
        onPreviewJsonChanged: window.preview = JSON.parse(previewJson)
    }

    CloudModelController {
        id: cloud
        services: window.services
    }

    CloudModelDialog {
        id: cloudDialog
        controller: cloud
        folder: window.folder
        folderTitle: organize.folderTitle
        onClosed: if (cloud.optedIn && cloud.configured && !organize.usedCloud) organize.analyze(window.folder, true)
    }

    onClosing: organize.cancel()

    ColumnLayout {
        anchors.fill: parent
        anchors.margins: 12
        spacing: 8

        RowLayout {
            Layout.fillWidth: true
            Label {
                Layout.fillWidth: true
                wrapMode: Text.Wrap
                text: organize.running ? organize.status
                      : organize.status + (organize.usedCloud ? "  " + qsTr("(with your cloud model)")
                                                               : "  " + qsTr("(on this computer)"))
            }
            Button {
                text: cloud.configured && cloud.optedIn ? qsTr("Cloud Model: On for This Folder") : qsTr("Cloud Model…")
                onClicked: cloudDialog.open()
            }
            Button {
                text: organize.running ? qsTr("Stop") : qsTr("Analyze Again")
                onClicked: organize.running ? organize.cancel() : organize.analyze(window.folder, true)
            }
        }
        ProgressBar {
            Layout.fillWidth: true
            visible: organize.running
            indeterminate: organize.progress < 0
            value: Math.max(0, organize.progress)
        }
        Label {
            Layout.fillWidth: true
            visible: text !== ""
            wrapMode: Text.Wrap
            text: organize.notes
            opacity: 0.8
        }
        Label {
            Layout.fillWidth: true
            visible: organize.errorMessage !== ""
            wrapMode: Text.Wrap
            text: organize.errorMessage
        }

        SplitView {
            Layout.fillWidth: true
            Layout.fillHeight: true

            ScrollView {
                SplitView.preferredWidth: parent.width * 0.6
                clip: true
                ColumnLayout {
                    width: parent.parent.width - 16
                    spacing: 12
                    Repeater {
                        model: window.sections
                        delegate: ColumnLayout {
                            id: section
                            required property var modelData
                            readonly property var items: window.inSection(modelData.key)
                            Layout.fillWidth: true
                            visible: items.length > 0
                            spacing: 2
                            RowLayout {
                                Label {
                                    text: section.modelData.title + " (" + section.items.length + ")"
                                    font.bold: true
                                    Layout.fillWidth: true
                                }
                                Button {
                                    flat: true
                                    text: qsTr("All")
                                    onClicked: organize.setSectionAccepted(section.modelData.key, true)
                                }
                                Button {
                                    flat: true
                                    text: qsTr("None")
                                    onClicked: organize.setSectionAccepted(section.modelData.key, false)
                                }
                            }
                            Repeater {
                                model: section.items
                                delegate: RowLayout {
                                    required property var modelData
                                    Layout.fillWidth: true
                                    CheckBox {
                                        checked: modelData.accepted
                                        onToggled: organize.setAccepted(modelData.id, checked)
                                        Accessible.name: modelData.name + " " + modelData.change
                                    }
                                    ColumnLayout {
                                        Layout.fillWidth: true
                                        spacing: 0
                                        Label {
                                            Layout.fillWidth: true
                                            elide: Text.ElideMiddle
                                            text: modelData.name + "  " + modelData.change
                                        }
                                        Label {
                                            Layout.fillWidth: true
                                            elide: Text.ElideRight
                                            text: modelData.reason + " · " + modelData.source
                                            opacity: 0.65
                                            font.pixelSize: Qt.application.font.pixelSize * 0.9
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }

            ColumnLayout {
                SplitView.fillWidth: true
                spacing: 4
                Label {
                    text: qsTr("Before → after")
                    font.bold: true
                }
                Label {
                    visible: window.preview.length === 0
                    wrapMode: Text.Wrap
                    Layout.fillWidth: true
                    text: qsTr("Tick suggestions to see what would change.")
                    opacity: 0.7
                }
                ListView {
                    Layout.fillWidth: true
                    Layout.fillHeight: true
                    clip: true
                    model: window.preview
                    delegate: ColumnLayout {
                        required property var modelData
                        width: ListView.view.width
                        spacing: 0
                        Label {
                            Layout.fillWidth: true
                            elide: Text.ElideMiddle
                            text: modelData.before
                            font.strikeout: modelData.after === "" || modelData.after !== modelData.before
                            opacity: 0.7
                        }
                        Label {
                            Layout.fillWidth: true
                            elide: Text.ElideMiddle
                            text: (modelData.after === "" ? "→ " + qsTr("Trash") : "→ " + modelData.after)
                                  + (modelData.tags.length > 0 ? "   [" + modelData.tags.join(", ") + "]" : "")
                            color: modelData.after === "" ? "#c62828" : "#2e7d32"
                        }
                    }
                }
            }
        }

        RowLayout {
            Layout.fillWidth: true
            Button {
                text: qsTr("Accept All")
                enabled: window.suggestions.length > 0
                onClicked: organize.setSectionAccepted("", true)
            }
            Button {
                text: qsTr("Reject All")
                enabled: organize.acceptedCount > 0
                onClicked: organize.setSectionAccepted("", false)
            }
            Label {
                Layout.fillWidth: true
                horizontalAlignment: Text.AlignRight
                text: qsTr("One Undo puts everything back.")
                opacity: 0.7
            }
            Button {
                text: organize.acceptedCount === 1 ? qsTr("Apply 1 Change") : qsTr("Apply %1 Changes").arg(organize.acceptedCount)
                highlighted: true
                enabled: organize.acceptedCount > 0 && !organize.running
                onClicked: {
                    const plan = organize.acceptedPlan()
                    if (plan !== "") {
                        window.operations.applyOrganization(plan, window.folder)
                        window.close()
                    }
                }
            }
        }
    }
}
