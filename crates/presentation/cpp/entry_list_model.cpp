#include "presentation/cpp/entry_list_model.h"

#include <QtCore/QDateTime>

namespace {

QString toQString(const rust::String& value)
{
    return QString::fromUtf8(value.data(), static_cast<qsizetype>(value.size()));
}

QVariant modifiedValue(const EntryRow& row)
{
    if (!row.has_modified) {
        return {};
    }
    return QDateTime::fromMSecsSinceEpoch(row.modified_ms);
}

} // namespace

EntryListModel::EntryListModel(QObject* parent)
    : QAbstractListModel(parent)
{
}

int EntryListModel::rowCount(const QModelIndex& parent) const
{
    return parent.isValid() ? 0 : static_cast<int>(m_rows.size());
}

QVariant EntryListModel::data(const QModelIndex& index, int role) const
{
    if (!index.isValid() || index.row() < 0 || index.row() >= rowCount()) {
        return {};
    }
    const EntryRow& row = m_rows[static_cast<std::size_t>(index.row())];
    switch (role) {
    case Qt::DisplayRole:
    case NameRole:
        return toQString(row.name);
    case LocationRole:
        return toQString(row.location);
    case IconNameRole:
        return toQString(row.icon_name);
    case SizeTextRole:
        return toQString(row.size_text);
    case ModifiedRole:
        return modifiedValue(row);
    case IsContainerRole:
        return row.is_container;
    case IsHiddenRole:
        return row.is_hidden;
    default:
        return {};
    }
}

QHash<int, QByteArray> EntryListModel::roleNames() const
{
    return {
        { NameRole, "name" },
        { LocationRole, "location" },
        { IconNameRole, "iconName" },
        { SizeTextRole, "sizeText" },
        { ModifiedRole, "modified" },
        { IsContainerRole, "isContainer" },
        { IsHiddenRole, "isHidden" },
    };
}

void EntryListModel::replaceAll(rust::Vec<EntryRow> rows)
{
    beginResetModel();
    m_rows = std::move(rows);
    endResetModel();
}

void EntryListModel::insertRowAt(std::size_t index, EntryRow row)
{
    if (index > m_rows.size()) {
        return;
    }
    const int position = static_cast<int>(index);
    beginInsertRows(QModelIndex(), position, position);
    rows_insert(m_rows, index, std::move(row));
    endInsertRows();
}

void EntryListModel::removeRowAt(std::size_t index)
{
    if (index >= m_rows.size()) {
        return;
    }
    const int position = static_cast<int>(index);
    beginRemoveRows(QModelIndex(), position, position);
    rows_remove(m_rows, index);
    endRemoveRows();
}

void EntryListModel::replaceRowAt(std::size_t index, EntryRow row)
{
    if (index >= m_rows.size()) {
        return;
    }
    rows_replace(m_rows, index, std::move(row));
    const QModelIndex changed = this->index(static_cast<int>(index));
    Q_EMIT dataChanged(changed, changed);
}

const rust::Vec<EntryRow>& EntryListModel::rows() const
{
    return m_rows;
}

std::unique_ptr<EntryListModel> newEntryListModel()
{
    return std::make_unique<EntryListModel>();
}
