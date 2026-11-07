import QtQuick
import QtQuick.Controls
import QtQuick.Layouts
import FinderMaxxing

// Convert or resize images, and view or remove their metadata. Both changes
// are undoable jobs of the window's OperationsController.
Dialog {
    id: dialog

    required property ServicesHolder services
    required property OperationsController operations
    property var locations: []

    readonly property int jpegIndex: 0
    readonly property int defaultQuality: 85
    readonly property int defaultPercent: 50
    readonly property int defaultLongestSide: 1920

    function openFor(items, showMetadata) {
        locations = items
        tabs.currentIndex = showMetadata ? 1 : 0
        if (items.length > 0) {
            metadata.load(items[0])
        }
        open()
    }

    parent: Overlay.overlay
    anchors.centerIn: parent
    width: Math.min(560, parent.width - 32)
    height: Math.min(560, parent.height - 32)
    modal: true
    title: locations.length === 1 ? qsTr("Image Tools") : qsTr("Image Tools — %1 images").arg(locations.length)
    standardButtons: Dialog.Close

    ImageMetadataController {
        id: metadata
        services: dialog.services
    }

    ColumnLayout {
        anchors.fill: parent
        spacing: 8

        TabBar {
            id: tabs
            Layout.fillWidth: true
            TabButton { text: qsTr("Convert & Resize") }
            TabButton { text: qsTr("Metadata") }
        }

        StackLayout {
            Layout.fillWidth: true
            Layout.fillHeight: true
            currentIndex: tabs.currentIndex

            ColumnLayout {
                spacing: 8
                RowLayout {
                    Label { text: qsTr("Format") }
                    ComboBox {
                        id: format
                        model: dialog.operations.imageFormats()
                        Accessible.name: qsTr("Image format")
                    }
                }
                ButtonGroup { id: sizing }
                RadioButton {
                    id: keep
                    text: qsTr("Keep the size")
                    checked: true
                    ButtonGroup.group: sizing
                }
                RowLayout {
                    RadioButton {
                        id: percent
                        text: qsTr("Scale to")
                        ButtonGroup.group: sizing
                    }
                    SpinBox {
                        id: percentValue
                        from: 1
                        to: 100
                        value: dialog.defaultPercent
                        enabled: percent.checked
                        Accessible.name: qsTr("Percent")
                    }
                    Label { text: "%" }
                }
                RowLayout {
                    RadioButton {
                        id: fit
                        text: qsTr("Longest side at most")
                        ButtonGroup.group: sizing
                    }
                    SpinBox {
                        id: fitValue
                        from: 16
                        to: 20000
                        stepSize: 10
                        editable: true
                        value: dialog.defaultLongestSide
                        enabled: fit.checked
                        Accessible.name: qsTr("Pixels")
                    }
                    Label { text: qsTr("pixels") }
                }
                RowLayout {
                    visible: format.currentIndex === dialog.jpegIndex
                    Label { text: qsTr("Quality") }
                    Slider {
                        id: quality
                        from: 10
                        to: 100
                        stepSize: 1
                        value: dialog.defaultQuality
                        Accessible.name: qsTr("JPEG quality")
                    }
                    Label { text: Math.round(quality.value) }
                }
                Label {
                    Layout.fillWidth: true
                    wrapMode: Text.Wrap
                    font.pixelSize: Qt.application.font.pixelSize * 0.9
                    text: qsTr("New files are saved next to the originals, which stay as they are. Images are never enlarged.")
                }
                Item { Layout.fillHeight: true }
                Button {
                    Layout.alignment: Qt.AlignRight
                    text: qsTr("Convert")
                    highlighted: true
                    onClicked: {
                        const mode = percent.checked ? OperationsController.ScalePercent
                                   : fit.checked ? OperationsController.FitWithin : OperationsController.KeepSize
                        const size = percent.checked ? percentValue.value : fitValue.value
                        dialog.operations.convertImages(dialog.locations, format.currentIndex, mode, size, Math.round(quality.value))
                        dialog.close()
                    }
                }
            }

            ColumnLayout {
                spacing: 8
                Label {
                    Layout.fillWidth: true
                    wrapMode: Text.Wrap
                    visible: metadata.revealsLocation || metadata.revealsIdentity
                    font.bold: true
                    text: metadata.revealsLocation && metadata.revealsIdentity
                          ? qsTr("This photo shows where it was taken and who took it.")
                          : metadata.revealsLocation ? qsTr("This photo shows where it was taken.")
                                                     : qsTr("This photo shows who took it or the camera's serial number.")
                }
                Label {
                    visible: metadata.busy
                    text: qsTr("Reading…")
                }
                Label {
                    visible: !metadata.busy && fieldList.count === 0 && metadata.errorMessage === ""
                    text: qsTr("No metadata.")
                }
                Label {
                    Layout.fillWidth: true
                    visible: metadata.errorMessage !== ""
                    wrapMode: Text.Wrap
                    text: metadata.errorMessage
                }
                ListView {
                    id: fieldList
                    Layout.fillWidth: true
                    Layout.fillHeight: true
                    clip: true
                    model: JSON.parse(metadata.fieldsJson)
                    delegate: RowLayout {
                        required property var modelData
                        width: ListView.view.width
                        Label {
                            Layout.preferredWidth: 180
                            text: (modelData.sensitive ? "⚠ " : "") + modelData.tag
                            elide: Text.ElideRight
                            font.bold: modelData.sensitive
                        }
                        Label {
                            Layout.fillWidth: true
                            text: modelData.value
                            elide: Text.ElideRight
                        }
                        Accessible.name: modelData.tag + ": " + modelData.value
                    }
                }
                Button {
                    Layout.alignment: Qt.AlignRight
                    text: dialog.locations.length === 1 ? qsTr("Remove Metadata")
                                                        : qsTr("Remove Metadata from %1 Images").arg(dialog.locations.length)
                    onClicked: {
                        dialog.operations.cleanMetadata(dialog.locations)
                        dialog.close()
                    }
                    ToolTip.visible: hovered
                    ToolTip.text: qsTr("Pixels and colour profiles stay; the original goes to the Trash.")
                }
            }
        }
    }
}
