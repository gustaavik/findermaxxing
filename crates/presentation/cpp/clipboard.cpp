#include "presentation/cpp/clipboard.h"

#include <QtCore/QMimeData>
#include <QtCore/QUrl>
#include <QtGui/QClipboard>
#include <QtGui/QGuiApplication>

void setClipboardUrls(const rust::Vec<rust::String>& urls)
{
    QList<QUrl> list;
    QStringList localPaths;
    for (const rust::String& url : urls) {
        const QUrl parsed = QUrl::fromEncoded(QByteArray(url.data(), static_cast<qsizetype>(url.size())));
        list.append(parsed);
        localPaths.append(parsed.toLocalFile());
    }
    auto* mime = new QMimeData;
    mime->setUrls(list);
    // Plain text too, so pasting into a terminal or editor yields the paths.
    mime->setText(localPaths.join(QLatin1Char('\n')));
    QGuiApplication::clipboard()->setMimeData(mime);
}

rust::Vec<rust::String> clipboardUrls()
{
    rust::Vec<rust::String> result;
    const QMimeData* mime = QGuiApplication::clipboard()->mimeData();
    if (mime == nullptr || !mime->hasUrls()) {
        return result;
    }
    for (const QUrl& url : mime->urls()) {
        const QByteArray encoded = url.toEncoded();
        result.push_back(rust::String(encoded.constData(), static_cast<std::size_t>(encoded.size())));
    }
    return result;
}
