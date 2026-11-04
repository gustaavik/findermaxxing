import QtQuick
import QtQuick.Controls
import QtQuick.Layouts
import FinderMaxxing

// Filters and saved searches, shown while the folder view is searching.
// Calls `changed()` whenever a filter changes; the view re-runs the search.
ToolBar {
    id: bar

    required property FolderViewController controller

    readonly property int kind: kindBox.currentValue
    readonly property int date: dateBox.currentValue
    readonly property int size: sizeBox.currentValue
    /// Search what files say (the AI index) instead of their names.
    readonly property bool contents: contentsButton.checked

    signal changed
    signal done

    // Mirror a saved search's filters when one is run.
    Connections {
        target: bar.controller
        function onSearchKindChanged() { kindBox.currentIndex = kindBox.indexOfValue(bar.controller.searchKind) }
        function onSearchDateChanged() { dateBox.currentIndex = dateBox.indexOfValue(bar.controller.searchDate) }
        function onSearchSizeChanged() { sizeBox.currentIndex = sizeBox.indexOfValue(bar.controller.searchSize) }
    }

    RowLayout {
        anchors.fill: parent
        anchors.leftMargin: 8
        anchors.rightMargin: 8

        Label {
            text: qsTr("Searching “%1”").arg(bar.controller.title)
            font.bold: true
        }
        ButtonGroup { id: modeGroup }
        Button {
            text: qsTr("Names")
            checkable: true
            checked: !bar.controller.contentSearch
            ButtonGroup.group: modeGroup
            onClicked: bar.changed()
            ToolTip.visible: hovered
            ToolTip.text: qsTr("Match words in file names")
        }
        Button {
            id: contentsButton
            text: qsTr("Contents (AI)")
            checkable: true
            checked: bar.controller.contentSearch
            ButtonGroup.group: modeGroup
            onClicked: bar.changed()
            ToolTip.visible: hovered
            ToolTip.text: qsTr("Find files by what they say or show, e.g. “invoices from last spring”")
        }
        ComboBox {
            id: kindBox
            visible: !bar.contents
            textRole: "text"
            valueRole: "value"
            Accessible.name: qsTr("Kind")
            model: [
                { text: qsTr("Any kind"), value: FolderViewController.AnyKind },
                { text: qsTr("Folders"), value: FolderViewController.Folders },
                { text: qsTr("Images"), value: FolderViewController.Images },
                { text: qsTr("PDFs"), value: FolderViewController.Pdfs },
                { text: qsTr("Movies"), value: FolderViewController.Videos },
                { text: qsTr("Music"), value: FolderViewController.Music },
                { text: qsTr("Text and code"), value: FolderViewController.Text },
                { text: qsTr("Archives"), value: FolderViewController.Archives },
                { text: qsTr("Documents"), value: FolderViewController.Documents }
            ]
            onActivated: bar.changed()
        }
        ComboBox {
            id: dateBox
            visible: !bar.contents
            textRole: "text"
            valueRole: "value"
            Accessible.name: qsTr("Modified")
            model: [
                { text: qsTr("Any date"), value: FolderViewController.AnyDate },
                { text: qsTr("Past day"), value: FolderViewController.PastDay },
                { text: qsTr("Past week"), value: FolderViewController.PastWeek },
                { text: qsTr("Past month"), value: FolderViewController.PastMonth },
                { text: qsTr("Past year"), value: FolderViewController.PastYear }
            ]
            onActivated: bar.changed()
        }
        ComboBox {
            id: sizeBox
            visible: !bar.contents
            textRole: "text"
            valueRole: "value"
            Accessible.name: qsTr("Size")
            model: [
                { text: qsTr("Any size"), value: FolderViewController.AnySize },
                { text: qsTr("Under 100 KB"), value: FolderViewController.Small },
                { text: qsTr("100 KB – 10 MB"), value: FolderViewController.Medium },
                { text: qsTr("10 MB – 1 GB"), value: FolderViewController.Large },
                { text: qsTr("Over 1 GB"), value: FolderViewController.Huge }
            ]
            onActivated: bar.changed()
        }
        Label {
            Layout.fillWidth: true
            text: bar.controller.searchStatus
            elide: Text.ElideRight
            color: palette.placeholderText
        }
        Button {
            text: qsTr("Save…")
            visible: !bar.contents
            enabled: bar.controller.searching
            onClicked: saveDialog.open()
        }
        Button {
            id: savedButton
            text: qsTr("Saved ▾")
            onClicked: {
                bar.controller.refreshSavedSearches()
                savedMenu.popup(savedButton, 0, savedButton.height)
            }
        }
        Button {
            text: qsTr("Done")
            onClicked: bar.done()
        }
    }

    Menu {
        id: savedMenu

        MenuItem {
            text: qsTr("No saved searches")
            enabled: false
            visible: bar.controller.savedSearchNames.length === 0
            height: visible ? implicitHeight : 0
        }
        Instantiator {
            model: bar.controller.savedSearchNames
            delegate: MenuItem {
                required property int index
                required property string modelData
                text: modelData
                onTriggered: bar.controller.runSavedSearch(index)
            }
            onObjectAdded: (index, item) => savedMenu.insertItem(index + 1, item)
            onObjectRemoved: (index, item) => savedMenu.removeItem(item)
        }
        MenuSeparator {}
        Menu {
            id: deleteMenu
            title: qsTr("Delete")
            enabled: bar.controller.savedSearchNames.length > 0
            Instantiator {
                model: bar.controller.savedSearchNames
                delegate: MenuItem {
                    required property int index
                    required property string modelData
                    text: modelData
                    onTriggered: bar.controller.deleteSavedSearch(index)
                }
                onObjectAdded: (index, item) => deleteMenu.insertItem(index, item)
                onObjectRemoved: (index, item) => deleteMenu.removeItem(item)
            }
        }
    }

    Dialog {
        id: saveDialog

        parent: Overlay.overlay
        anchors.centerIn: parent
        modal: true
        title: qsTr("Save Search")
        standardButtons: Dialog.Save | Dialog.Cancel
        onAboutToShow: {
            nameField.text = bar.controller.searchText
            nameField.selectAll()
            nameField.forceActiveFocus()
        }
        onAccepted: bar.controller.saveSearch(nameField.text)

        TextField {
            id: nameField
            implicitWidth: 300
            placeholderText: qsTr("Name")
            Accessible.name: qsTr("Saved search name")
            onAccepted: saveDialog.accept()
        }
    }
}
