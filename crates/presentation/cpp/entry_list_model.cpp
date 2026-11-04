#include "presentation/cpp/entry_list_model.h"

#include <QtCore/QDateTime>
#include <QtCore/QSet>

#include <algorithm>

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
    if (!index.isValid() || !isValidRow(index.row())) {
        return {};
    }
    const auto row = static_cast<std::size_t>(index.row());
    const EntryRow& entry = m_rows[row];
    switch (role) {
    case Qt::DisplayRole:
    case NameRole:
        return toQString(entry.name);
    case LocationRole:
        return toQString(entry.location);
    case IconNameRole:
        return toQString(entry.icon_name);
    case SizeTextRole:
        return toQString(entry.size_text);
    case ModifiedRole:
        return modifiedValue(entry);
    case IsContainerRole:
        return entry.is_container;
    case IsHiddenRole:
        return entry.is_hidden;
    case SelectedRole:
        return bool(m_selected[row]);
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
        { SelectedRole, "selected" },
    };
}

void EntryListModel::replaceAll(rust::Vec<EntryRow> rows)
{
    const QStringList previouslySelected = m_selectedCount > 0 ? selectedLocations() : QStringList();
    beginResetModel();
    m_rows = std::move(rows);
    m_selected.assign(m_rows.size(), false);
    m_selectedCount = 0;
    m_anchor = -1;
    endResetModel();
    restoreSelection(previouslySelected);
    Q_EMIT selectionChanged();
}

void EntryListModel::insertRowAt(std::size_t index, EntryRow row)
{
    if (index > m_rows.size()) {
        return;
    }
    const int position = static_cast<int>(index);
    beginInsertRows(QModelIndex(), position, position);
    rows_insert(m_rows, index, std::move(row));
    m_selected.insert(m_selected.begin() + static_cast<std::ptrdiff_t>(index), false);
    if (m_anchor >= position) {
        ++m_anchor;
    }
    endInsertRows();
}

void EntryListModel::removeRowAt(std::size_t index)
{
    if (index >= m_rows.size()) {
        return;
    }
    const int position = static_cast<int>(index);
    const bool wasSelected = m_selected[index];
    beginRemoveRows(QModelIndex(), position, position);
    rows_remove(m_rows, index);
    m_selected.erase(m_selected.begin() + static_cast<std::ptrdiff_t>(index));
    if (m_anchor == position) {
        m_anchor = -1;
    } else if (m_anchor > position) {
        --m_anchor;
    }
    endRemoveRows();
    if (wasSelected) {
        --m_selectedCount;
        Q_EMIT selectionChanged();
    }
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

int EntryListModel::selectedCount() const
{
    return m_selectedCount;
}

void EntryListModel::selectOnly(int row)
{
    clearSelection();
    if (isValidRow(row)) {
        setSelected(row, true);
        m_anchor = row;
        Q_EMIT selectionChanged();
    }
}

void EntryListModel::toggleSelected(int row)
{
    if (!isValidRow(row)) {
        return;
    }
    setSelected(row, !m_selected[static_cast<std::size_t>(row)]);
    m_anchor = row;
    Q_EMIT selectionChanged();
}

void EntryListModel::extendSelectionTo(int row)
{
    if (!isValidRow(row)) {
        return;
    }
    const int anchor = isValidRow(m_anchor) ? m_anchor : row;
    const int first = std::min(anchor, row);
    const int last = std::max(anchor, row);
    for (int i = 0; i < rowCount(); ++i) {
        const bool inRange = i >= first && i <= last;
        if (m_selected[static_cast<std::size_t>(i)] != inRange) {
            m_selected[static_cast<std::size_t>(i)] = inRange;
            m_selectedCount += inRange ? 1 : -1;
        }
    }
    m_anchor = anchor;
    notifySelectionRange(0, rowCount() - 1);
    Q_EMIT selectionChanged();
}

void EntryListModel::selectAll()
{
    std::fill(m_selected.begin(), m_selected.end(), true);
    m_selectedCount = rowCount();
    notifySelectionRange(0, rowCount() - 1);
    Q_EMIT selectionChanged();
}

void EntryListModel::clearSelection()
{
    if (m_selectedCount == 0) {
        return;
    }
    std::fill(m_selected.begin(), m_selected.end(), false);
    m_selectedCount = 0;
    notifySelectionRange(0, rowCount() - 1);
    Q_EMIT selectionChanged();
}

bool EntryListModel::isSelected(int row) const
{
    return isValidRow(row) && m_selected[static_cast<std::size_t>(row)];
}

QStringList EntryListModel::selectedLocations() const
{
    QStringList locations;
    locations.reserve(m_selectedCount);
    for (std::size_t i = 0; i < m_rows.size(); ++i) {
        if (m_selected[i]) {
            locations.append(toQString(m_rows[i].location));
        }
    }
    return locations;
}

QString EntryListModel::locationAt(int row) const
{
    return isValidRow(row) ? toQString(m_rows[static_cast<std::size_t>(row)].location) : QString();
}

QString EntryListModel::nameAt(int row) const
{
    return isValidRow(row) ? toQString(m_rows[static_cast<std::size_t>(row)].name) : QString();
}

int EntryListModel::rowOfLocation(const QString& location) const
{
    const QByteArray wanted = location.toUtf8();
    for (std::size_t i = 0; i < m_rows.size(); ++i) {
        const rust::String& candidate = m_rows[i].location;
        if (candidate.size() == static_cast<std::size_t>(wanted.size())
            && std::equal(candidate.begin(), candidate.end(), wanted.begin())) {
            return static_cast<int>(i);
        }
    }
    return -1;
}

bool EntryListModel::isValidRow(int row) const
{
    return row >= 0 && row < rowCount();
}

void EntryListModel::setSelected(int row, bool selected)
{
    auto slot = m_selected.begin() + row;
    if (*slot == selected) {
        return;
    }
    *slot = selected;
    m_selectedCount += selected ? 1 : -1;
    notifySelectionRange(row, row);
}

void EntryListModel::notifySelectionRange(int first, int last)
{
    if (first > last) {
        return;
    }
    Q_EMIT dataChanged(index(first), index(last), { SelectedRole });
}

void EntryListModel::restoreSelection(const QStringList& locations)
{
    if (locations.isEmpty()) {
        return;
    }
    const QSet<QString> wanted(locations.begin(), locations.end());
    for (std::size_t i = 0; i < m_rows.size(); ++i) {
        if (wanted.contains(toQString(m_rows[i].location))) {
            m_selected[i] = true;
            ++m_selectedCount;
        }
    }
    notifySelectionRange(0, rowCount() - 1);
}

std::unique_ptr<EntryListModel> newEntryListModel()
{
    return std::make_unique<EntryListModel>();
}
