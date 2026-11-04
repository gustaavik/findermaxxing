#ifndef FINDERMAXXING_JOB_LIST_MODEL_H
#define FINDERMAXXING_JOB_LIST_MODEL_H
// Include guard instead of #pragma once: qmltyperegistrar reaches this header
// through two different paths (crate source and exported copy).

// Running file operations, one row per job. Small (a handful of rows), so it
// stores plain copies and looks rows up linearly.

#include <QtCore/QAbstractListModel>
#include <QtQml/qqmlregistration.h>

#include <cstdint>
#include <memory>
#include <vector>

#include "rust/cxx.h"
#include "presentation/src/rows_bridge.cxx.h"

class JobListModel : public QAbstractListModel
{
    Q_OBJECT
    QML_ELEMENT
    QML_UNCREATABLE("JobListModel is provided by OperationsController.jobs")

public:
    enum Role
    {
        JobIdRole = Qt::UserRole + 1,
        TitleRole,
        DetailRole,
        FractionRole,
        DeterminateRole,
    };
    Q_ENUM(Role)

    explicit JobListModel(QObject* parent = nullptr);

    int rowCount(const QModelIndex& parent = QModelIndex()) const override;
    QVariant data(const QModelIndex& index, int role) const override;
    QHash<int, QByteArray> roleNames() const override;

    void addJob(JobRow row);
    void updateProgress(std::int32_t jobId, rust::String detail, double fraction, bool determinate);
    void removeJob(std::int32_t jobId);

private:
    int rowOf(std::int32_t jobId) const;

    std::vector<JobRow> m_jobs;
};

std::unique_ptr<JobListModel> newJobListModel();

#endif // FINDERMAXXING_JOB_LIST_MODEL_H
