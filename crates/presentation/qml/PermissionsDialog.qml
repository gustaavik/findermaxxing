import QtQuick
import QtQuick.Controls
import QtQuick.Layouts
import FinderMaxxing

// Batch permission editing. Each checkbox is tri-state when the selected
// items differ; only bits the user changes are applied.
Dialog {
    id: dialog

    required property ServicesHolder services
    required property OperationsController operations
    property var locations: []

    readonly property var classes: [qsTr("Owner"), qsTr("Group"), qsTr("Everyone")]
    readonly property var accesses: [qsTr("Read"), qsTr("Write"), qsTr("Execute")]

    function openFor(items) {
        locations = items
        reach.currentIndex = 0
        controller.load(items)
        open()
    }

    function stateOf(bit) {
        return bit === "1" ? Qt.Checked : bit === "0" ? Qt.Unchecked : Qt.PartiallyChecked
    }

    // "+", "-" or "=" per bit: a change only where the box differs from what was read.
    function editString() {
        let edit = ""
        for (let i = 0; i < 9; ++i) {
            const box = boxes.itemAt(i)
            const original = stateOf(controller.bits.charAt(i))
            if (!box || box.checkState === original || box.checkState === Qt.PartiallyChecked) {
                edit += "="
            } else {
                edit += box.checkState === Qt.Checked ? "+" : "-"
            }
        }
        return edit
    }

    function showOctal(text) {
        const bits = controller.bitsOfOctal(text)
        if (bits.length === 9) {
            for (let i = 0; i < 9; ++i) {
                boxes.itemAt(i).checkState = stateOf(bits.charAt(i))
            }
        }
    }

    parent: Overlay.overlay
    anchors.centerIn: parent
    width: Math.min(460, parent.width - 32)
    modal: true
    title: locations.length === 1 ? qsTr("Permissions") : qsTr("Permissions of %1 items").arg(locations.length)

    PermissionsController {
        id: controller
        services: dialog.services
    }

    // Editing replaces the bindings; a new selection resets every box.
    Connections {
        target: controller
        function onBitsChanged() {
            for (let i = 0; i < 9; ++i) {
                const box = boxes.itemAt(i)
                if (box) {
                    box.checkState = dialog.stateOf(controller.bits.charAt(i))
                }
            }
        }
        function onOctalChanged() {
            octalField.text = controller.octal
        }
    }

    ColumnLayout {
        anchors.fill: parent
        spacing: 10

        GridLayout {
            columns: 4
            enabled: controller.editable
            Label { text: "" }
            Repeater {
                model: dialog.accesses
                delegate: Label {
                    required property string modelData
                    text: modelData
                    font.bold: true
                }
            }
            Repeater {
                id: boxes
                model: 9
                delegate: CheckBox {
                    required property int index
                    Layout.row: Math.floor(index / 3) + 1
                    Layout.column: index % 3 + 1
                    tristate: controller.bits.charAt(index) === "?"
                    checkState: dialog.stateOf(controller.bits.charAt(index))
                    Accessible.name: dialog.classes[Math.floor(index / 3)] + " " + dialog.accesses[index % 3]
                }
            }
            Repeater {
                model: dialog.classes
                delegate: Label {
                    required property int index
                    required property string modelData
                    Layout.row: index + 1
                    Layout.column: 0
                    text: modelData
                }
            }
        }

        RowLayout {
            enabled: controller.editable
            Label { text: qsTr("As a number") }
            TextField {
                id: octalField
                text: controller.octal
                placeholderText: qsTr("mixed")
                inputMethodHints: Qt.ImhDigitsOnly
                maximumLength: 4
                Layout.preferredWidth: 70
                onTextEdited: dialog.showOctal(text)
                Accessible.name: qsTr("Permissions as an octal number")
            }
            Label {
                text: controller.symbolic
                font.family: "monospace"
            }
        }

        RowLayout {
            visible: controller.hasFolders
            enabled: controller.editable
            Label { text: qsTr("Apply to") }
            ComboBox {
                id: reach
                Layout.fillWidth: true
                model: [qsTr("The selected items only"), qsTr("Everything inside too"),
                        qsTr("Files inside too"), qsTr("Folders inside too")]
                Accessible.name: qsTr("Apply to")
            }
        }

        Label {
            Layout.fillWidth: true
            visible: controller.errorMessage !== ""
            wrapMode: Text.Wrap
            text: controller.errorMessage
        }
    }

    footer: DialogButtonBox {
        Button {
            text: qsTr("Cancel")
            DialogButtonBox.buttonRole: DialogButtonBox.RejectRole
        }
        Button {
            text: qsTr("Apply")
            enabled: controller.editable && !controller.busy
            DialogButtonBox.buttonRole: DialogButtonBox.AcceptRole
        }
    }
    onAccepted: {
        const scopes = [OperationsController.SelectedOnly, OperationsController.AllInside,
                        OperationsController.FilesInside, OperationsController.FoldersInside]
        operations.changePermissions(locations, editString(), controller.hasFolders ? scopes[reach.currentIndex]
                                                                                    : OperationsController.SelectedOnly)
    }
}
