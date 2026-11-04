import QtQuick
import QtQuick.Controls
import QtQuick.Layouts
import FinderMaxxing

// Running file operations with progress and a cancel button each.
Pane {
    id: panel

    required property OperationsController operations

    readonly property int spacingUnit: 6

    visible: jobs.count > 0
    padding: spacingUnit * 2

    ColumnLayout {
        anchors.fill: parent
        spacing: panel.spacingUnit * 2

        Repeater {
            id: jobs
            model: panel.operations.jobs

            delegate: RowLayout {
                id: job

                required property int jobId
                required property string title
                required property string detail
                required property real fraction
                required property bool determinate

                Layout.fillWidth: true
                spacing: panel.spacingUnit * 2

                ColumnLayout {
                    Layout.fillWidth: true
                    spacing: panel.spacingUnit / 2

                    Label {
                        Layout.fillWidth: true
                        text: job.title
                        elide: Text.ElideMiddle
                    }
                    ProgressBar {
                        Layout.fillWidth: true
                        indeterminate: !job.determinate
                        value: job.fraction
                        Accessible.name: job.title
                    }
                    Label {
                        Layout.fillWidth: true
                        text: job.detail
                        elide: Text.ElideMiddle
                        font.pointSize: Qt.application.font.pointSize * 0.85
                        color: palette.placeholderText
                    }
                }

                ToolButton {
                    text: "✕"
                    onClicked: panel.operations.cancelJob(job.jobId)
                    ToolTip.visible: hovered
                    ToolTip.text: qsTr("Stop")
                    Accessible.name: qsTr("Stop %1").arg(job.title)
                }
            }
        }
    }
}
