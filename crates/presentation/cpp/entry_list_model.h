#ifndef FINDERMAXXING_ENTRY_LIST_MODEL_H
#define FINDERMAXXING_ENTRY_LIST_MODEL_H
// Include guard instead of #pragma once: qmltyperegistrar reaches this header
// through two different paths (crate source and exported copy).

// List model shim (ADR 0004). Rows live in a rust::Vec owned here; Rust
// computes every edit and calls the matching method, which wraps the edit in
// the begin/end notifications Qt views need.
//
// Selection is view state and lives here too (ADR 0009): it follows rows
// through inserts/removes and survives resets by location.

#include <QtCore/QAbstractListModel>
#include <QtCore/QStringList>
#include <QtQml/qqmlregistration.h>

#include <cstddef>
#include <memory>
#include <vector>

#include "rust/cxx.h"
#include "presentation/src/rows_bridge.cxx.h"

class EntryListModel : public QAbstractListModel
{
    Q_OBJECT
    QML_ELEMENT
    QML_UNCREATABLE("EntryListModel is provided by FolderViewController.model")
    Q_PROPERTY(int selectedCount READ selectedCount NOTIFY selectionChanged)

public:
    enum Role
    {
        NameRole = Qt::UserRole + 1,
        LocationRole,
        IconNameRole,
        SizeTextRole,
        ModifiedRole,
        IsContainerRole,
        IsHiddenRole,
        SelectedRole,
        FolderTextRole,
    };
    Q_ENUM(Role)

    explicit EntryListModel(QObject* parent = nullptr);

    int rowCount(const QModelIndex& parent = QModelIndex()) const override;
    QVariant data(const QModelIndex& index, int role) const override;
    QHash<int, QByteArray> roleNames() const override;

    void replaceAll(rust::Vec<EntryRow> rows);
    void insertRowAt(std::size_t index, EntryRow row);
    void removeRowAt(std::size_t index);
    void replaceRowAt(std::size_t index, EntryRow row);
    const rust::Vec<EntryRow>& rows() const;

    int selectedCount() const;
    Q_INVOKABLE void selectOnly(int row);
    Q_INVOKABLE void toggleSelected(int row);
    Q_INVOKABLE void extendSelectionTo(int row);
    Q_INVOKABLE void selectAll();
    Q_INVOKABLE void clearSelection();
    Q_INVOKABLE bool isSelected(int row) const;
    Q_INVOKABLE QStringList selectedLocations() const;
    Q_INVOKABLE QString locationAt(int row) const;
    Q_INVOKABLE QString nameAt(int row) const;
    Q_INVOKABLE bool isContainerAt(int row) const;
    Q_INVOKABLE int rowOfLocation(const QString& location) const;

Q_SIGNALS:
    void selectionChanged();

private:
    bool isValidRow(int row) const;
    void setSelected(int row, bool selected);
    void notifySelectionRange(int first, int last);
    void restoreSelection(const QStringList& locations);

    rust::Vec<EntryRow> m_rows;
    std::vector<bool> m_selected;
    int m_selectedCount = 0;
    int m_anchor = -1;
};

std::unique_ptr<EntryListModel> newEntryListModel();

#endif // FINDERMAXXING_ENTRY_LIST_MODEL_H
