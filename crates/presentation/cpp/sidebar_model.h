#ifndef FINDERMAXXING_SIDEBAR_MODEL_H
#define FINDERMAXXING_SIDEBAR_MODEL_H
// Include guard instead of #pragma once: qmltyperegistrar reaches this header
// through two different paths (crate source and exported copy).

// The sidebar's rows. Small and rebuilt as a whole by SidebarController.

#include <QtCore/QAbstractListModel>
#include <QtQml/qqmlregistration.h>

#include <memory>

#include "rust/cxx.h"
#include "presentation/src/rows_bridge.cxx.h"

class SidebarModel : public QAbstractListModel
{
    Q_OBJECT
    QML_ELEMENT
    QML_UNCREATABLE("SidebarModel is provided by SidebarController.model")

public:
    enum Role
    {
        KindRole = Qt::UserRole + 1,
        LabelRole,
        DetailRole,
        LocationRole,
        IconNameRole,
        AccountIdRole,
    };
    Q_ENUM(Role)

    explicit SidebarModel(QObject* parent = nullptr);

    int rowCount(const QModelIndex& parent = QModelIndex()) const override;
    QVariant data(const QModelIndex& index, int role) const override;
    QHash<int, QByteArray> roleNames() const override;

    void replaceAll(rust::Vec<SidebarRow> rows);

private:
    rust::Vec<SidebarRow> m_rows;
};

std::unique_ptr<SidebarModel> newSidebarModel();

#endif // FINDERMAXXING_SIDEBAR_MODEL_H
