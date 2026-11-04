#include "presentation/cpp/sidebar_model.h"

namespace {

QString toQString(const rust::String& value)
{
    return QString::fromUtf8(value.data(), static_cast<qsizetype>(value.size()));
}

} // namespace

SidebarModel::SidebarModel(QObject* parent)
    : QAbstractListModel(parent)
{
}

int SidebarModel::rowCount(const QModelIndex& parent) const
{
    return parent.isValid() ? 0 : static_cast<int>(m_rows.size());
}

QVariant SidebarModel::data(const QModelIndex& index, int role) const
{
    if (!index.isValid() || index.row() < 0 || index.row() >= rowCount()) {
        return {};
    }
    const SidebarRow& row = m_rows[static_cast<std::size_t>(index.row())];
    switch (role) {
    case KindRole:
        return row.kind;
    case Qt::DisplayRole:
    case LabelRole:
        return toQString(row.label);
    case DetailRole:
        return toQString(row.detail);
    case LocationRole:
        return toQString(row.location);
    case IconNameRole:
        return toQString(row.icon_name);
    case AccountIdRole:
        return QVariant::fromValue(static_cast<qint64>(row.account_id));
    default:
        return {};
    }
}

QHash<int, QByteArray> SidebarModel::roleNames() const
{
    return {
        { KindRole, "kind" },
        { LabelRole, "label" },
        { DetailRole, "detail" },
        { LocationRole, "location" },
        { IconNameRole, "iconName" },
        { AccountIdRole, "accountId" },
    };
}

void SidebarModel::replaceAll(rust::Vec<SidebarRow> rows)
{
    beginResetModel();
    m_rows = std::move(rows);
    endResetModel();
}

std::unique_ptr<SidebarModel> newSidebarModel()
{
    return std::make_unique<SidebarModel>();
}
