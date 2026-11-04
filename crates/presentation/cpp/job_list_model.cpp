#include "presentation/cpp/job_list_model.h"

namespace {

QString toQString(const rust::String& value)
{
    return QString::fromUtf8(value.data(), static_cast<qsizetype>(value.size()));
}

} // namespace

JobListModel::JobListModel(QObject* parent)
    : QAbstractListModel(parent)
{
}

int JobListModel::rowCount(const QModelIndex& parent) const
{
    return parent.isValid() ? 0 : static_cast<int>(m_jobs.size());
}

QVariant JobListModel::data(const QModelIndex& index, int role) const
{
    if (!index.isValid() || index.row() < 0 || index.row() >= rowCount()) {
        return {};
    }
    const JobRow& job = m_jobs[static_cast<std::size_t>(index.row())];
    switch (role) {
    case JobIdRole:
        return job.id;
    case Qt::DisplayRole:
    case TitleRole:
        return toQString(job.title);
    case DetailRole:
        return toQString(job.detail);
    case FractionRole:
        return job.fraction;
    case DeterminateRole:
        return job.determinate;
    default:
        return {};
    }
}

QHash<int, QByteArray> JobListModel::roleNames() const
{
    return {
        { JobIdRole, "jobId" },
        { TitleRole, "title" },
        { DetailRole, "detail" },
        { FractionRole, "fraction" },
        { DeterminateRole, "determinate" },
    };
}

void JobListModel::addJob(JobRow row)
{
    const int position = rowCount();
    beginInsertRows(QModelIndex(), position, position);
    m_jobs.push_back(std::move(row));
    endInsertRows();
}

void JobListModel::updateProgress(std::int32_t jobId, rust::String detail, double fraction, bool determinate)
{
    const int position = rowOf(jobId);
    if (position < 0) {
        return;
    }
    JobRow& job = m_jobs[static_cast<std::size_t>(position)];
    job.detail = std::move(detail);
    job.fraction = fraction;
    job.determinate = determinate;
    const QModelIndex changed = index(position);
    Q_EMIT dataChanged(changed, changed);
}

void JobListModel::removeJob(std::int32_t jobId)
{
    const int position = rowOf(jobId);
    if (position < 0) {
        return;
    }
    beginRemoveRows(QModelIndex(), position, position);
    m_jobs.erase(m_jobs.begin() + position);
    endRemoveRows();
}

int JobListModel::rowOf(std::int32_t jobId) const
{
    for (std::size_t i = 0; i < m_jobs.size(); ++i) {
        if (m_jobs[i].id == jobId) {
            return static_cast<int>(i);
        }
    }
    return -1;
}

std::unique_ptr<JobListModel> newJobListModel()
{
    return std::make_unique<JobListModel>();
}
