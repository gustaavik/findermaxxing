import QtQuick
import QtQuick.Controls
import QtQuick.Layouts
import FinderMaxxing

// Asks what to do when an item with the same name exists. Visibility follows
// OperationsController.conflictPending, so the dialog never closes itself.
Dialog {
    id: dialog

    required property OperationsController operations

    readonly property int preferredWidth: 460

    function answer(choice) {
        operations.answerConflict(choice, applyToAll.checked ? OperationsController.AllRemaining
                                                              : OperationsController.ThisItem)
        applyToAll.checked = false
    }

    parent: Overlay.overlay
    anchors.centerIn: parent
    width: Math.min(preferredWidth, parent.width - 32)
    modal: true
    closePolicy: Popup.NoAutoClose
    visible: operations.conflictPending
    title: qsTr("Replace existing item?")

    ColumnLayout {
        anchors.fill: parent
        spacing: 10

        Label {
            Layout.fillWidth: true
            text: dialog.operations.conflictTitle
            wrapMode: Text.Wrap
            font.bold: true
        }
        Label {
            Layout.fillWidth: true
            text: dialog.operations.conflictDetail
            wrapMode: Text.Wrap
        }
        CheckBox {
            id: applyToAll
            text: qsTr("Apply to all remaining conflicts")
        }
    }

    footer: DialogButtonBox {
        Button {
            text: qsTr("Stop")
            onClicked: dialog.answer(OperationsController.Stop)
        }
        Button {
            text: qsTr("Skip")
            onClicked: dialog.answer(OperationsController.Skip)
        }
        Button {
            text: qsTr("Keep Both")
            onClicked: dialog.answer(OperationsController.KeepBoth)
        }
        Button {
            text: qsTr("Replace")
            highlighted: true
            onClicked: dialog.answer(OperationsController.Replace)
        }
    }
}
