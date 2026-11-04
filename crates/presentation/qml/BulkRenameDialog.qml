import QtQuick
import QtQuick.Controls
import QtQuick.Layouts
import FinderMaxxing

// "Rename N Items": choose a rule, watch the live preview, apply as one
// undoable step. Every control change re-plans in Rust immediately.
Dialog {
    id: dialog

    required property ServicesHolder services
    required property OperationsController operations

    property var locations: []

    readonly property int previewHeight: 240
    readonly property int preferredWidth: 640

    function openFor(items) {
        locations = items
        renamer.load(items)
        reconfigure()
        open()
    }

    function reconfigure() {
        const scope = includeExtension.checked ? BulkRenameController.NameAndExtension : BulkRenameController.NameOnly
        switch (mode.currentIndex) {
        case 0:
            renamer.useReplace(findField.text, replaceField.text,
                               regexBox.checked ? BulkRenameController.RegularExpression : BulkRenameController.PlainText,
                               matchCaseBox.checked ? BulkRenameController.MatchCase : BulkRenameController.IgnoreCase,
                               scope)
            break
        case 1:
            renamer.useAdd(addField.text, addPlacement.currentValue)
            break
        case 2:
            renamer.useFormat(baseField.text, startBox.value, digitsBox.value)
            break
        case 3:
            renamer.useCaseChange(caseStyle.currentValue, scope)
            break
        }
    }

    parent: Overlay.overlay
    anchors.centerIn: parent
    width: Math.min(preferredWidth, parent.width - 32)
    modal: true
    title: locations.length === 1 ? qsTr("Rename 1 Item") : qsTr("Rename %1 Items").arg(locations.length)

    BulkRenameController {
        id: renamer
        services: dialog.services
        onFinished: error => {
            if (error !== "") {
                dialog.operations.errorMessage = error
            }
            dialog.operations.refreshHistory()
            dialog.close()
        }
    }

    ColumnLayout {
        anchors.fill: parent
        spacing: 10

        ComboBox {
            id: mode
            Layout.fillWidth: true
            model: [qsTr("Replace Text"), qsTr("Add Text"), qsTr("Format"), qsTr("Change Case")]
            Accessible.name: qsTr("Rename method")
            onActivated: dialog.reconfigure()
        }

        StackLayout {
            Layout.fillWidth: true
            currentIndex: mode.currentIndex

            GridLayout {
                columns: 2
                Label { text: qsTr("Find:") }
                TextField {
                    id: findField
                    Layout.fillWidth: true
                    Accessible.name: qsTr("Find")
                    onTextEdited: dialog.reconfigure()
                }
                Label { text: qsTr("Replace with:") }
                TextField {
                    id: replaceField
                    Layout.fillWidth: true
                    placeholderText: regexBox.checked ? qsTr("Use $1, $2 … for groups") : ""
                    Accessible.name: qsTr("Replace with")
                    onTextEdited: dialog.reconfigure()
                }
                Item { width: 1 }
                RowLayout {
                    CheckBox {
                        id: regexBox
                        text: qsTr("Regular expression")
                        onToggled: dialog.reconfigure()
                    }
                    CheckBox {
                        id: matchCaseBox
                        text: qsTr("Match case")
                        onToggled: dialog.reconfigure()
                    }
                }
            }

            GridLayout {
                columns: 2
                Label { text: qsTr("Text:") }
                TextField {
                    id: addField
                    Layout.fillWidth: true
                    Accessible.name: qsTr("Text to add")
                    onTextEdited: dialog.reconfigure()
                }
                Label { text: qsTr("Where:") }
                ComboBox {
                    id: addPlacement
                    textRole: "text"
                    valueRole: "value"
                    model: [
                        { text: qsTr("Before name"), value: BulkRenameController.BeforeName },
                        { text: qsTr("After name"), value: BulkRenameController.AfterName }
                    ]
                    onActivated: dialog.reconfigure()
                }
            }

            GridLayout {
                columns: 2
                Label { text: qsTr("Name:") }
                TextField {
                    id: baseField
                    Layout.fillWidth: true
                    text: qsTr("File")
                    Accessible.name: qsTr("Base name")
                    onTextEdited: dialog.reconfigure()
                }
                Label { text: qsTr("Start at:") }
                SpinBox {
                    id: startBox
                    from: 0
                    to: 999999
                    value: 1
                    editable: true
                    onValueModified: dialog.reconfigure()
                }
                Label { text: qsTr("Digits:") }
                SpinBox {
                    id: digitsBox
                    from: 1
                    to: 9
                    value: 3
                    onValueModified: dialog.reconfigure()
                }
            }

            RowLayout {
                Label { text: qsTr("Change to:") }
                ComboBox {
                    id: caseStyle
                    textRole: "text"
                    valueRole: "value"
                    model: [
                        { text: qsTr("lowercase"), value: BulkRenameController.Lowercase },
                        { text: qsTr("UPPERCASE"), value: BulkRenameController.Uppercase },
                        { text: qsTr("Title Case"), value: BulkRenameController.TitleCase }
                    ]
                    onActivated: dialog.reconfigure()
                }
            }
        }

        CheckBox {
            id: includeExtension
            visible: mode.currentIndex === 0 || mode.currentIndex === 3
            text: qsTr("Include extensions")
            onToggled: dialog.reconfigure()
        }

        Label {
            Layout.fillWidth: true
            visible: renamer.ruleError !== ""
            text: renamer.ruleError
            color: "#d9534f"
            wrapMode: Text.Wrap
        }

        Frame {
            Layout.fillWidth: true
            Layout.preferredHeight: dialog.previewHeight
            padding: 0

            ListView {
                anchors.fill: parent
                clip: true
                model: renamer.originalNames.length
                ScrollBar.vertical: ScrollBar {}
                Accessible.name: qsTr("Preview")
                delegate: RowLayout {
                    required property int index
                    readonly property string problem: renamer.problems[index] ?? ""
                    width: ListView.view.width
                    spacing: 8

                    Label {
                        Layout.preferredWidth: parent.width * 0.4
                        leftPadding: 8
                        text: renamer.originalNames[index] ?? ""
                        elide: Text.ElideMiddle
                    }
                    Label { text: "→" }
                    Label {
                        Layout.fillWidth: true
                        text: renamer.newNames[index] ?? ""
                        elide: Text.ElideMiddle
                        font.bold: text !== renamer.originalNames[index]
                    }
                    Label {
                        rightPadding: 8
                        visible: parent.problem !== ""
                        text: parent.problem
                        color: "#d9534f"
                    }
                }
            }
        }

        Label {
            Layout.fillWidth: true
            text: renamer.summary
            wrapMode: Text.Wrap
        }
    }

    footer: DialogButtonBox {
        Button {
            text: qsTr("Cancel")
            DialogButtonBox.buttonRole: DialogButtonBox.RejectRole
        }
        Button {
            text: qsTr("Rename")
            highlighted: true
            enabled: renamer.canApply && !renamer.busy
            onClicked: renamer.apply()
        }
    }
}
