import QtQuick
import QtQuick.Controls
import QtQuick.Layouts
import FinderMaxxing

// Confirms transfers that may be billed. Visibility follows
// OperationsController.costPending, so the dialog never closes itself.
Dialog {
    id: dialog

    required property OperationsController operations

    readonly property int preferredWidth: 460

    function answer(choice) {
        operations.answerCost(choice, stopAsking.checked ? OperationsController.StopAsking
                                                         : OperationsController.AskAgain)
        stopAsking.checked = false
    }

    parent: Overlay.overlay
    anchors.centerIn: parent
    width: Math.min(preferredWidth, parent.width - 32)
    modal: true
    closePolicy: Popup.NoAutoClose
    visible: operations.costPending
    title: qsTr("This transfer may cost money")

    ColumnLayout {
        anchors.fill: parent
        spacing: 10

        Label {
            Layout.fillWidth: true
            text: dialog.operations.costTitle
            wrapMode: Text.Wrap
            font.bold: true
        }
        Label {
            Layout.fillWidth: true
            text: dialog.operations.costDetail
            wrapMode: Text.Wrap
        }
        CheckBox {
            id: stopAsking
            text: qsTr("Don't ask again for this account")
        }
    }

    footer: DialogButtonBox {
        Button {
            text: qsTr("Cancel")
            onClicked: dialog.answer(OperationsController.CancelTransfer)
        }
        Button {
            text: qsTr("Continue")
            highlighted: true
            onClicked: dialog.answer(OperationsController.Continue)
        }
    }
}
