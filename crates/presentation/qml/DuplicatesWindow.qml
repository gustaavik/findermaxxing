import QtQuick
import QtQuick.Controls
import QtQuick.Layouts
import FinderMaxxing

// Find Duplicates: search folders, review groups (the suggested keeper is
// unticked), move the ticked copies to the Trash. Undo brings them back.
ApplicationWindow {
    id: window

    required property ServicesHolder services
    required property OperationsController operations
    property var folders: []
    property var groups: []
    // location → ticked, for the copies to remove.
    property var ticked: ({})
    property int tickedCount: 0

    function openFor(folder, label) {
        folders = [{ location: folder, label: label }]
        groups = []
        show()
        raise()
        requestActivate()
    }

    function tickDefaults() {
        const next = {}
        for (const group of groups) {
            group.items.forEach((item, index) => next[item.location] = index > 0)
        }
        ticked = next
        countTicked()
    }

    function countTicked() {
        tickedCount = Object.values(ticked).filter(t => t).length
    }

    function removeTicked() {
        const chosen = Object.keys(ticked).filter(location => ticked[location])
        if (chosen.length > 0) {
            operations.moveToTrash(chosen)
            controller.forget(chosen)
        }
    }

    width: 760
    height: 560
    title: qsTr("Find Duplicates")

    DuplicatesController {
        id: controller
        services: window.services
        onGroupsJsonChanged: {
            window.groups = JSON.parse(groupsJson)
            window.tickDefaults()
        }
    }

    onClosing: controller.cancel()

    ColumnLayout {
        anchors.fill: parent
        anchors.margins: 12
        spacing: 8

        RowLayout {
            Layout.fillWidth: true
            Label {
                Layout.fillWidth: true
                elide: Text.ElideMiddle
                text: qsTr("In: %1").arg(window.folders.map(f => f.label).join(", "))
            }
            CheckBox {
                id: hidden
                text: qsTr("Hidden files")
            }
            CheckBox {
                id: similar
                text: qsTr("Look-alike photos")
                ToolTip.visible: hovered
                ToolTip.text: qsTr("Also finds resized or re-saved copies of the same photo. Slower: every image is looked at.")
            }
            Label { text: qsTr("At least") }
            SpinBox {
                id: minimum
                from: 0
                to: 1048576
                value: 1
                editable: true
                Accessible.name: qsTr("Minimum size in KB")
            }
            Label { text: "KB" }
            Button {
                text: controller.running ? qsTr("Stop") : qsTr("Find")
                highlighted: !controller.running
                onClicked: controller.running ? controller.cancel()
                                              : controller.start(window.folders.map(f => f.location), minimum.value,
                                                                 hidden.checked, similar.checked)
            }
        }

        ProgressBar {
            Layout.fillWidth: true
            visible: controller.running
            indeterminate: controller.progress < 0
            value: Math.max(0, controller.progress)
        }
        Label {
            Layout.fillWidth: true
            elide: Text.ElideMiddle
            text: controller.running ? controller.status : controller.summary
        }
        Label {
            Layout.fillWidth: true
            visible: controller.errorMessage !== ""
            wrapMode: Text.Wrap
            text: controller.errorMessage
        }

        ListView {
            id: list
            Layout.fillWidth: true
            Layout.fillHeight: true
            clip: true
            spacing: 10
            model: window.groups
            ScrollBar.vertical: ScrollBar {}
            delegate: ColumnLayout {
                id: group
                required property var modelData
                width: list.width - 12
                spacing: 2
                Label {
                    font.bold: true
                    text: (modelData.matching === "similar" ? qsTr("Look alike") : qsTr("Identical"))
                          + " — " + qsTr("%1 can be freed").arg(modelData.reclaimable)
                }
                Repeater {
                    model: group.modelData.items
                    delegate: RowLayout {
                        required property var modelData
                        required property int index
                        Layout.fillWidth: true
                        CheckBox {
                            checked: window.ticked[modelData.location] === true
                            onToggled: {
                                window.ticked[modelData.location] = checked
                                window.countTicked()
                            }
                            Accessible.name: qsTr("Remove %1").arg(modelData.name)
                        }
                        Label {
                            text: modelData.name + (index === 0 ? "  " + qsTr("(suggested to keep)") : "")
                            font.italic: index === 0
                        }
                        Label {
                            Layout.fillWidth: true
                            text: modelData.folder
                            elide: Text.ElideMiddle
                            opacity: 0.7
                        }
                        Label { text: modelData.size }
                        TapHandler {
                            onDoubleTapped: window.operations.open(modelData.location)
                        }
                    }
                }
            }
        }

        RowLayout {
            Layout.fillWidth: true
            Label {
                Layout.fillWidth: true
                text: qsTr("Ticked copies go to the Trash; Undo brings them back.")
                opacity: 0.7
            }
            Button {
                text: qsTr("Move %1 to Trash").arg(window.tickedCount)
                enabled: window.tickedCount > 0 && !controller.running
                onClicked: window.removeTicked()
            }
        }
    }
}
