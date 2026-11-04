import QtQuick
import QtQuick.Controls
import QtQuick.Layouts
import QtQuick.Pdf
import QtMultimedia
import FinderMaxxing

// Shows whatever PreviewController prepared for the current item. Each kind
// has its own component; the Loader destroys the previous one (stopping any
// playing media) when the kind changes.
Pane {
    id: pane

    required property PreviewController preview

    readonly property string monospaceFamily: Qt.platform.os === "osx" ? "Menlo" : "monospace"
    readonly property int largeIconSize: 96
    readonly property int spacingUnit: 8

    padding: spacingUnit * 1.5

    ColumnLayout {
        anchors.fill: parent
        spacing: pane.spacingUnit

        Loader {
            Layout.fillWidth: true
            Layout.fillHeight: true
            sourceComponent: {
                switch (pane.preview.kind) {
                case PreviewController.Image: return imageView
                case PreviewController.Pdf: return pdfView
                case PreviewController.Video: return videoView
                case PreviewController.Audio: return audioView
                case PreviewController.Text: return textView
                case PreviewController.Archive: return archiveView
                case PreviewController.Nothing: return emptyView
                default: return iconView
                }
            }
        }

        Label {
            Layout.fillWidth: true
            visible: pane.preview.kind !== PreviewController.Nothing
            text: pane.preview.title
            font.bold: true
            elide: Text.ElideMiddle
            Accessible.name: text
        }
        Label {
            Layout.fillWidth: true
            visible: text !== ""
            text: pane.preview.subtitle
            color: palette.placeholderText
        }
        Label {
            Layout.fillWidth: true
            visible: pane.preview.hasModified
            text: qsTr("Modified %1").arg(new Date(pane.preview.modifiedMs).toLocaleString(Qt.locale(), Locale.ShortFormat))
            color: palette.placeholderText
        }
        Label {
            Layout.fillWidth: true
            visible: text !== ""
            text: pane.preview.note
            wrapMode: Text.Wrap
            color: palette.placeholderText
            font.italic: true
        }
    }

    Component {
        id: emptyView
        Label {
            text: qsTr("Select an item to preview it.")
            horizontalAlignment: Text.AlignHCenter
            verticalAlignment: Text.AlignVCenter
            color: palette.placeholderText
        }
    }

    Component {
        id: iconView
        Item {
            FileIcon {
                anchors.centerIn: parent
                width: pane.largeIconSize
                height: pane.largeIconSize
                kind: pane.preview.kind === PreviewController.Folder ? "folder" : "file"
            }
        }
    }

    Component {
        id: imageView
        Image {
            source: pane.preview.sourceUrl
            fillMode: Image.PreserveAspectFit
            asynchronous: true
            autoTransform: true
            mipmap: true
            // Decode at display size, not at the camera's 50 megapixels.
            sourceSize.width: width * Screen.devicePixelRatio
            sourceSize.height: height * Screen.devicePixelRatio
            Accessible.role: Accessible.Graphic
            Accessible.name: pane.preview.title

            BusyIndicator {
                anchors.centerIn: parent
                running: parent.status === Image.Loading
            }
            Label {
                anchors.centerIn: parent
                visible: parent.status === Image.Error
                text: qsTr("This image can't be displayed.")
                color: palette.placeholderText
            }
        }
    }

    Component {
        id: pdfView
        PdfMultiPageView {
            document: PdfDocument {
                source: pane.preview.sourceUrl
            }
        }
    }

    Component {
        id: videoView
        ColumnLayout {
            MediaPlayer {
                id: videoPlayer
                source: pane.preview.sourceUrl
                videoOutput: videoOutput
                audioOutput: AudioOutput {}
            }
            VideoOutput {
                id: videoOutput
                Layout.fillWidth: true
                Layout.fillHeight: true
            }
            MediaControls {
                Layout.fillWidth: true
                player: videoPlayer
            }
        }
    }

    Component {
        id: audioView
        ColumnLayout {
            MediaPlayer {
                id: audioPlayer
                source: pane.preview.sourceUrl
                audioOutput: AudioOutput {}
            }
            Item {
                Layout.fillHeight: true
            }
            MediaControls {
                Layout.fillWidth: true
                player: audioPlayer
            }
            Item {
                Layout.fillHeight: true
            }
        }
    }

    Component {
        id: textView
        ScrollView {
            TextArea {
                readOnly: true
                selectByMouse: true
                textFormat: TextEdit.RichText
                text: pane.preview.html
                wrapMode: TextEdit.NoWrap
                font.family: pane.monospaceFamily
                Accessible.name: qsTr("Contents of %1").arg(pane.preview.title)
            }
        }
    }

    Component {
        id: archiveView
        ListView {
            clip: true
            model: pane.preview.archiveLines
            ScrollBar.vertical: ScrollBar {}
            delegate: Label {
                required property string modelData
                width: ListView.view.width
                text: modelData
                elide: Text.ElideMiddle
                font.family: pane.monospaceFamily
            }
        }
    }

    component MediaControls: RowLayout {
        id: controls

        required property MediaPlayer player

        Button {
            text: controls.player.playbackState === MediaPlayer.PlayingState ? qsTr("Pause") : qsTr("Play")
            onClicked: controls.player.playbackState === MediaPlayer.PlayingState ? controls.player.pause()
                                                                                    : controls.player.play()
        }
        Slider {
            Layout.fillWidth: true
            from: 0
            to: Math.max(1, controls.player.duration)
            value: controls.player.position
            enabled: controls.player.seekable
            onMoved: controls.player.position = value
            Accessible.name: qsTr("Position")
        }
    }
}
