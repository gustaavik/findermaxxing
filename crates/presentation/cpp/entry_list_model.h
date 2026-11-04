#ifndef FINDERMAXXING_ENTRY_LIST_MODEL_H
#define FINDERMAXXING_ENTRY_LIST_MODEL_H
// Include guard instead of #pragma once: qmltyperegistrar reaches this header
// through two different paths (crate source and exported copy).

// List model shim (ADR 0004). Rows live in a rust::Vec owned here; Rust
// computes every edit and calls the matching method, which wraps the edit in
// the begin/end notifications Qt views need.

#include <QtCore/QAbstractListModel>
#include <QtQml/qqmlregistration.h>

#include <cstddef>
#include <memory>

#include "rust/cxx.h"
#include "presentation/src/rows_bridge.cxx.h"

class EntryListModel : public QAbstractListModel
{
    Q_OBJECT
    QML_ELEMENT
    QML_UNCREATABLE("EntryListModel is provided by FolderViewController.model")

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

private:
    rust::Vec<EntryRow> m_rows;
};

std::unique_ptr<EntryListModel> newEntryListModel();

#endif // FINDERMAXXING_ENTRY_LIST_MODEL_H
