import Testing
import Foundation
import CoreGraphics
import AppKit
@testable import JZOffice

struct JZOfficeTests {

    private func samplePath(_ name: String) -> URL {
        // viewer-macos/Tests/JZOfficeTests/JZOfficeTests.swift -> repo root -> demo/src/main/assets/samples/<name>
        let testFile = URL(fileURLWithPath: #filePath)
        let repoRoot = testFile
            .deletingLastPathComponent()
            .deletingLastPathComponent()
            .deletingLastPathComponent()
            .deletingLastPathComponent()
        return repoRoot.appendingPathComponent("demo/src/main/assets/samples/\(name)")
    }

    @Test("OfficeMetadata 读取 DOCX / PPTX / XLSX")
    func metadataAllFormats() throws {
        let docxURL = samplePath("sample.docx")
        let pptxURL = samplePath("sample.pptx")
        let xlsxURL = samplePath("sample.xlsx")

        guard FileManager.default.fileExists(atPath: docxURL.path) else {
            return
        }

        let docxMeta = try OfficeMetadata.read(from: docxURL)
        #expect(docxMeta.format == .docx)
        #expect(docxMeta.pageCount == 1)
        #expect(docxMeta.widthPt > 0)
        #expect(docxMeta.heightPt == 0)
        #expect(docxMeta.sheetNames.isEmpty)

        let pptxMeta = try OfficeMetadata.read(from: pptxURL)
        #expect(pptxMeta.format == .pptx)
        #expect(pptxMeta.pageCount >= 1)
        #expect(pptxMeta.widthPt > 0)
        #expect(pptxMeta.heightPt > 0)
        #expect(pptxMeta.sheetNames.isEmpty)

        let xlsxMeta = try OfficeMetadata.read(from: xlsxURL)
        #expect(xlsxMeta.format == .xlsx)
        #expect(xlsxMeta.pageCount == xlsxMeta.sheetNames.count)
        #expect(!xlsxMeta.sheetNames.isEmpty)
        #expect(xlsxMeta.widthPt >= 0)
        #expect(xlsxMeta.heightPt == 0)
    }

    @Test("OfficeThumbnail 离屏渲染 DOCX / PPTX / XLSX 非空白且遵守边界约束")
    func thumbnailAllFormats() throws {
        for name in ["sample.docx", "sample.pptx", "sample.xlsx"] {
            let url = samplePath(name)
            guard FileManager.default.fileExists(atPath: url.path) else { continue }
            let cgImg = try OfficeThumbnail.renderCGImage(from: url, maxSize: CGSize(width: 240, height: 240))
            #expect(cgImg.width > 0)
            #expect(cgImg.height > 0)
            #expect(cgImg.width <= 240)
            #expect(cgImg.height <= 240)
            #expect(hasNonWhitePixel(cgImg), "Thumbnail for \(name) should contain non-white content")
        }
    }

    @Test("OfficePreviewController 页码与工作表状态同步")
    @MainActor
    func previewControllerAndSheetSync() {
        let controller = OfficePreviewController()
        var callbackCount = 0
        var lastPage = -1
        var lastTotal = -1
        let id = controller.addOnPageChangeListener { page, count in
            callbackCount += 1
            lastPage = page
            lastTotal = count
        }
        #expect(callbackCount == 1)
        #expect(lastPage == 0)

        controller.updateDocumentState(page: 1, count: 3, sheets: ["概览", "明细", "汇总"])
        #expect(lastPage == 1)
        #expect(lastTotal == 3)

        controller.selectSheet(2)
        #expect(controller.currentPage == 2)
        #expect(lastPage == 2)

        controller.removeOnPageChangeListener(id)
    }

    @Test("ImageStore 从 ZIP 包内解码图片并驱逐缓存")
    func imageStoreDecodeAndCacheEviction() throws {
        let pptxURL = samplePath("sample.pptx")
        guard FileManager.default.fileExists(atPath: pptxURL.path) else { return }
        let doc = try NativeCore.parseDocument(at: pptxURL.path)
        let store = ImageStore(filePath: pptxURL.path)
        defer { store.release() }

        var decodedCount = 0
        for page in doc.pages {
            for el in page.elements where el.type == .image {
                if let part = el.image, store.loadSync(part: part, maxPixelBudget: 1_000_000) != nil {
                    decodedCount += 1
                }
            }
        }
        #expect(decodedCount >= 1)
    }

    @Test("按页缩略图与 PDF / PNG 矢量导出校验")
    func perPageThumbnailAndExporters() throws {
        let pptxURL = samplePath("sample.pptx")
        guard FileManager.default.fileExists(atPath: pptxURL.path) else { return }

        let slide2 = try OfficeThumbnail.renderPageCGImage(
            from: pptxURL,
            pageNumber: 2,
            maxSize: CGSize(width: 240, height: 240)
        )
        #expect(slide2.width > 0)
        #expect(hasNonWhitePixel(slide2))

        let tempDir = FileManager.default.temporaryDirectory
            .appendingPathComponent("jz-office-export-\(UUID().uuidString)", isDirectory: true)
        try FileManager.default.createDirectory(at: tempDir, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: tempDir) }

        for name in ["sample.docx", "sample.pptx", "sample.xlsx"] {
            let src = samplePath(name)
            guard FileManager.default.fileExists(atPath: src.path) else { continue }
            let pdfOut = tempDir.appendingPathComponent("\(name).pdf")
            try OfficeExporter.exportPDF(from: src, to: pdfOut)
            let pdfData = try Data(contentsOf: pdfOut)
            #expect(pdfData.count > 256)
        }

        let pngOut = tempDir.appendingPathComponent("slide1.png")
        try OfficeExporter.exportPNG(
            from: pptxURL,
            pageNumber: 1,
            maxSize: CGSize(width: 480, height: 480),
            to: pngOut
        )
        let pngData = try Data(contentsOf: pngOut)
        #expect(pngData.count > 256)
    }

    @Test("DocumentTextLayout 空间索引、命中测试与选区计算")
    func documentTextLayoutAndSelection() throws {
        let docxURL = samplePath("sample.docx")
        guard FileManager.default.fileExists(atPath: docxURL.path) else { return }

        let doc = try NativeCore.parseDocument(at: docxURL.path)
        let layout = DocumentTextLayout.build(from: doc)
        #expect(!layout.blocks.isEmpty)

        // 验证至少有一个 block 包含有效行与文本
        guard let firstBlock = layout.blocks.first(where: { !$0.lines.isEmpty && !$0.fullText.isEmpty }) else {
            Issue.record("未在 sample.docx 中解析出任何文本块")
            return
        }
        #expect(!firstBlock.fullText.isEmpty)

        // 1. 命中测试：取第一行的中心点
        let firstLine = firstBlock.lines[0]
        let hitPoint = CGPoint(
            x: firstBlock.bounds.minX + firstLine.rect.midX,
            y: firstBlock.bounds.minY + firstLine.rect.midY
        )
        let hitPos = layout.hitTest(docPoint: hitPoint)
        #expect(hitPos != nil)
        #expect(hitPos?.blockIndex == firstBlock.blockIndex)

        // 2. 选区与文本提取：选取该 block 的前几个字符
        let selEndChar = min(5, (firstBlock.fullText as NSString).length)
        let sel = TextSelection(
            anchor: TextPosition(blockIndex: firstBlock.blockIndex, charIndex: 0),
            focus: TextPosition(blockIndex: firstBlock.blockIndex, charIndex: selEndChar)
        )
        #expect(!sel.isEmpty)
        let extracted = layout.selectedText(for: sel)
        #expect(!extracted.isEmpty)
        #expect(extracted.count <= selEndChar)

        // 3. 高亮矩形计算
        let rects = layout.selectionRects(for: sel)
        #expect(!rects.isEmpty)
        for r in rects {
            #expect(r.width > 0)
            #expect(r.height > 0)
        }

        // 4. 双击选词
        if let wordSel = layout.wordRange(at: TextPosition(blockIndex: firstBlock.blockIndex, charIndex: 0)) {
            #expect(!wordSel.isEmpty)
            #expect(wordSel.start.charIndex <= wordSel.end.charIndex)
        }

        // 5. 全选
        if let allSel = layout.allTextRange() {
            #expect(!allSel.isEmpty)
            let allText = layout.selectedText(for: allSel)
            #expect(!allText.isEmpty)
        }

        // 6. PPTX 幻灯片多元素文本排版与选区验证
        let pptxURL = samplePath("sample.pptx")
        if FileManager.default.fileExists(atPath: pptxURL.path) {
            let pptxDoc = try NativeCore.parseDocument(at: pptxURL.path)
            let pptxLayout = DocumentTextLayout.build(from: pptxDoc)
            #expect(!pptxLayout.blocks.isEmpty)
            if let pptxSel = pptxLayout.allTextRange() {
                let pptxText = pptxLayout.selectedText(for: pptxSel)
                #expect(!pptxText.isEmpty)
            }
        }

        // 7. 针对 PPTX 表格与两端对齐段落的文本空间索引与选区校验
        let testPptxJson = """
        {
            "schemaVersion": 9,
            "kind": "PPTX",
            "width": 720,
            "pages": [{
                "width": 720,
                "height": 405,
                "elements": [
                    {
                        "type": "TABLE",
                        "width": 300,
                        "rows": [
                            [[{"runs": [{"text": "PPTX表格文字1"}]}], [{"runs": [{"text": "PPTX表格文字2"}]}]]
                        ]
                    },
                    {
                        "type": "TEXT",
                        "width": 120,
                        "paragraphs": [
                            {
                                "alignment": 3,
                                "runs": [{"text": "这是一个用于测试两端对齐排版以及末行不对齐行为的长文本测试段落。"}]
                            }
                        ]
                    }
                ]
            }]
        }
        """.data(using: .utf8)!
        let testPptxDoc = try JSONDecoder().decode(OfficeDocument.self, from: testPptxJson)
        let testPptxLayout = DocumentTextLayout.build(from: testPptxDoc)
        #expect(testPptxLayout.blocks.count >= 2)
        #expect(testPptxLayout.blocks[0].fullText.contains("PPTX表格文字1"))
        if let allSel = testPptxLayout.allTextRange() {
            let rects = testPptxLayout.selectionRects(for: allSel)
            #expect(!rects.isEmpty)
            for r in rects {
                #expect(r.width > 0)
                #expect(r.height > 0)
            }
        }
    }

    @Test("NativeCore 全文检索接口支持 DOCX / PPTX / XLSX 并返回 UTF-16 偏移与摘要")
    func fullTextSearchAllFormats() throws {
        let docxURL = samplePath("sample.docx")
        let pptxURL = samplePath("sample.pptx")
        let xlsxURL = samplePath("sample.xlsx")

        guard FileManager.default.fileExists(atPath: docxURL.path) else {
            return
        }

        // 空查询返回空数组
        let emptyMatches = try NativeCore.searchDocument(at: docxURL.path, query: "")
        #expect(emptyMatches.isEmpty)

        // DOCX 检索
        let docxDoc = try NativeCore.parseDocument(at: docxURL.path)
        let firstWord = docxDoc.blocks
            .flatMap(\.paragraphs)
            .flatMap(\.runs)
            .map(\.text)
            .first { !$0.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty }?
            .trimmingCharacters(in: .whitespacesAndNewlines) ?? ""
        if !firstWord.isEmpty {
            let query = String(firstWord.prefix(2))
            let matches = try NativeCore.searchDocument(at: docxURL.path, query: query)
            #expect(!matches.isEmpty)
            #expect(matches[0].pageIndex == 0)
            #expect(matches[0].endChar > matches[0].startChar)
            #expect(!matches[0].snippet.isEmpty)
        }

        // PPTX 检索
        if FileManager.default.fileExists(atPath: pptxURL.path) {
            let pptxDoc = try NativeCore.parseDocument(at: pptxURL.path)
            let slideWord = pptxDoc.pages
                .flatMap(\.elements)
                .flatMap(\.paragraphs)
                .flatMap(\.runs)
                .map(\.text)
                .first { !$0.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty }?
                .trimmingCharacters(in: .whitespacesAndNewlines) ?? ""
            if !slideWord.isEmpty {
                let query = String(slideWord.prefix(2))
                let matches = try NativeCore.searchDocument(at: pptxURL.path, query: query)
                #expect(!matches.isEmpty)
                #expect(matches[0].endChar > matches[0].startChar)
            }
        }

        // XLSX 检索
        if FileManager.default.fileExists(atPath: xlsxURL.path) {
            let xlsxDoc = try NativeCore.parseDocument(at: xlsxURL.path)
            let cellWord = xlsxDoc.sheets
                .flatMap(\.cells)
                .map(\.text)
                .first { !$0.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty }?
                .trimmingCharacters(in: .whitespacesAndNewlines) ?? ""
            if !cellWord.isEmpty {
                let query = String(cellWord.prefix(2))
                let matches = try NativeCore.searchDocument(at: xlsxURL.path, query: query)
                #expect(!matches.isEmpty)
                #expect(matches[0].row != nil)
                #expect(matches[0].column != nil)
            }
        }
    }

    @Test("CGElementDrawer 全文检索高亮矩形计算与行偏移缓存")
    func computeHighlightRectsAllFormats() throws {
        let docxURL = samplePath("sample.docx")
        let pptxURL = samplePath("sample.pptx")
        let xlsxURL = samplePath("sample.xlsx")

        guard FileManager.default.fileExists(atPath: docxURL.path) else { return }

        // 1. DOCX 高亮矩形计算与行偏移缓存
        let docxDoc = try NativeCore.parseDocument(at: docxURL.path)
        let offsets = CGElementDrawer.computeDocxBlockYOffsets(in: docxDoc)
        #expect(offsets.count == docxDoc.headerBlocks.count + docxDoc.blocks.count + docxDoc.footerBlocks.count)
        for i in 1..<offsets.count {
            #expect(offsets[i] >= offsets[i - 1])
        }

        let docxWord = docxDoc.blocks
            .flatMap(\.paragraphs)
            .flatMap(\.runs)
            .map(\.text)
            .first { $0.trimmingCharacters(in: .whitespacesAndNewlines).count >= 2 }?
            .trimmingCharacters(in: .whitespacesAndNewlines) ?? ""
        if !docxWord.isEmpty {
            let matches = try NativeCore.searchDocument(at: docxURL.path, query: String(docxWord.prefix(2)))
            if let first = matches.first {
                let rects = CGElementDrawer.computeHighlightRects(for: first, in: docxDoc, docxBlockYOffsets: offsets)
                #expect(!rects.isEmpty)
                for r in rects {
                    #expect(r.width > 0)
                    #expect(r.height > 0)
                }
            }
        }

        // 2. PPTX 高亮矩形计算
        if FileManager.default.fileExists(atPath: pptxURL.path) {
            let pptxDoc = try NativeCore.parseDocument(at: pptxURL.path)
            let slideWord = pptxDoc.pages
                .flatMap(\.elements)
                .flatMap(\.paragraphs)
                .flatMap(\.runs)
                .map(\.text)
                .first { $0.trimmingCharacters(in: .whitespacesAndNewlines).count >= 2 }?
                .trimmingCharacters(in: .whitespacesAndNewlines) ?? ""
            if !slideWord.isEmpty {
                let matches = try NativeCore.searchDocument(at: pptxURL.path, query: String(slideWord.prefix(2)))
                if let first = matches.first {
                    let rects = CGElementDrawer.computeHighlightRects(for: first, in: pptxDoc)
                    #expect(!rects.isEmpty)
                    for r in rects {
                        #expect(r.width > 0)
                        #expect(r.height > 0)
                    }
                }
            }
        }

        // 3. XLSX 高亮矩形计算
        if FileManager.default.fileExists(atPath: xlsxURL.path) {
            let xlsxDoc = try NativeCore.parseDocument(at: xlsxURL.path)
            let cellWord = xlsxDoc.sheets
                .flatMap(\.cells)
                .map(\.text)
                .first { $0.trimmingCharacters(in: .whitespacesAndNewlines).count >= 2 }?
                .trimmingCharacters(in: .whitespacesAndNewlines) ?? ""
            if !cellWord.isEmpty {
                let matches = try NativeCore.searchDocument(at: xlsxURL.path, query: String(cellWord.prefix(2)))
                if let first = matches.first {
                    let rects = CGElementDrawer.computeHighlightRects(for: first, in: xlsxDoc)
                    #expect(!rects.isEmpty)
                    for r in rects {
                        #expect(r.width > 0)
                        #expect(r.height > 0)
                    }
                }
            }
        }
    }

    @Test("OfficePreviewController 检索状态转换与环形遍历")
    @MainActor
    func searchControllerStateTransitionsAndCycling() {
        let controller = OfficePreviewController()
        #expect(!controller.isSearchPresented)
        #expect(controller.searchQuery == "")
        #expect(controller.searchResults.isEmpty)
        #expect(controller.currentMatchIndex == -1)

        // 展示 / 收起交互
        controller.presentSearch()
        #expect(controller.isSearchPresented)

        controller.dismissSearch()
        #expect(!controller.isSearchPresented)
        #expect(controller.searchQuery == "")

        // 模拟命中项并测试环形跳转
        let match1 = OfficeSearchMatch(
            pageIndex: 0, elementIndex: 0, paragraphIndex: 0,
            row: nil, column: nil, startChar: 0, endChar: 2, snippet: "测试"
        )
        let match2 = OfficeSearchMatch(
            pageIndex: 0, elementIndex: 1, paragraphIndex: 0,
            row: nil, column: nil, startChar: 5, endChar: 7, snippet: "测试2"
        )
        controller.searchResults = [match1, match2]
        controller.currentMatchIndex = 0

        // findNext 环形递增
        #expect(controller.findNext())
        #expect(controller.currentMatchIndex == 1)
        #expect(controller.findNext())
        #expect(controller.currentMatchIndex == 0)

        // findPrevious 环形递减
        #expect(controller.findPrevious())
        #expect(controller.currentMatchIndex == 1)
        #expect(controller.findPrevious())
        #expect(controller.currentMatchIndex == 0)

        // 清空检索
        controller.clearSearch()
        #expect(controller.searchQuery == "")
        #expect(controller.searchResults.isEmpty)
        #expect(controller.currentMatchIndex == -1)
        #expect(controller.targetHighlightRect == nil)
    }

    @Test("DOCX 与 PPTX 表格横向与纵向合并单元格解析、几何测量与绘制")
    func tableMergedCellsDecodingMeasurementAndDrawing() throws {
        // 1. JSON 解码 OfficeElement 携带 merges
        let json = """
        {
            "type": "TABLE",
            "width": 300,
            "columnWidths": [100, 100, 100],
            "merges": [
                {"startRow": 0, "startColumn": 0, "endRow": 0, "endColumn": 1},
                {"startRow": 1, "startColumn": 0, "endRow": 2, "endColumn": 0}
            ],
            "rows": [
                [
                    [{"runs": [{"text": "跨2列合并"}]}],
                    [],
                    [{"runs": [{"text": "普通格0-2"}]}]
                ],
                [
                    [{"runs": [{"text": "跨2行纵向合并"}]}],
                    [{"runs": [{"text": "普通格1-1"}]}],
                    [{"runs": [{"text": "普通格1-2"}]}]
                ],
                [
                    [],
                    [{"runs": [{"text": "普通格2-1"}]}],
                    [{"runs": [{"text": "普通格2-2"}]}]
                ]
            ]
        }
        """.data(using: .utf8)!

        let element = try JSONDecoder().decode(OfficeElement.self, from: json)
        #expect(element.type == .table)
        #expect(element.merges.count == 2)
        #expect(element.merges[0].startRow == 0 && element.merges[0].endColumn == 1)
        #expect(element.merges[1].startRow == 1 && element.merges[1].endRow == 2)

        // 2. findMerge 命中与未命中校验
        let m0 = CGElementDrawer.findMerge(element.merges, r: 0, c: 0)
        #expect(m0 != nil)
        #expect(m0?.endColumn == 1)

        let m01 = CGElementDrawer.findMerge(element.merges, r: 0, c: 1)
        #expect(m01 != nil)
        #expect(m01?.startColumn == 0)

        let mNone = CGElementDrawer.findMerge(element.merges, r: 0, c: 2)
        #expect(mNone == nil)

        let mV = CGElementDrawer.findMerge(element.merges, r: 2, c: 0)
        #expect(mV != nil)
        #expect(mV?.startRow == 1 && mV?.endRow == 2)

        // 3. 几何测量：cols, colX, rowTops
        let geom = CGElementDrawer.computeTableGeometry(element: element)
        #expect(geom.cols == 3)
        #expect(geom.colX.count == 4)
        #expect(geom.rowTops.count == 4)
        #expect(geom.colX[0] == 0)
        #expect(geom.colX[3] == 300)
        let totalH = CGElementDrawer.measureTableHeight(element: element)
        #expect(totalH >= 60)
        #expect(geom.rowTops[3] == totalH)

        // 4. 离屏绘制无崩溃且产生非空白像素
        let colorSpace = CGColorSpace(name: CGColorSpace.sRGB) ?? CGColorSpaceCreateDeviceRGB()
        var bytes = [UInt8](repeating: 255, count: 300 * Int(totalH) * 4)
        let ctx = CGContext(
            data: &bytes,
            width: 300,
            height: Int(totalH),
            bitsPerComponent: 8,
            bytesPerRow: 300 * 4,
            space: colorSpace,
            bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue
        )!
        CGElementDrawer.drawElement(in: ctx, element: element, images: [:])
        let img = ctx.makeImage()!
        #expect(hasNonWhitePixel(img))
    }

    private func hasNonWhitePixel(_ image: CGImage) -> Bool {
        let w = image.width
        let h = image.height
        guard w > 0, h > 0 else { return false }
        var bytes = [UInt8](repeating: 255, count: w * h * 4)
        let colorSpace = CGColorSpace(name: CGColorSpace.sRGB) ?? CGColorSpaceCreateDeviceRGB()
        guard let ctx = CGContext(
            data: &bytes,
            width: w,
            height: h,
            bitsPerComponent: 8,
            bytesPerRow: w * 4,
            space: colorSpace,
            bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue
        ) else {
            return false
        }
        ctx.draw(image, in: CGRect(x: 0, y: 0, width: w, height: h))
        for i in stride(from: 0, to: bytes.count, by: 16) {
            let r = bytes[i]
            let g = bytes[i + 1]
            let b = bytes[i + 2]
            if r < 248 || g < 248 || b < 248 {
                return true
            }
        }
        return false
    }

    /// 采样整张图片的全部像素，供「某类颜色必须出现」这类存在性断言使用。
    private func pixelSampler(for image: CGImage) -> [(r: UInt8, g: UInt8, b: UInt8, a: UInt8)]? {
        let w = image.width
        let h = image.height
        guard w > 0, h > 0 else { return nil }
        var bytes = [UInt8](repeating: 255, count: w * h * 4)
        let colorSpace = CGColorSpace(name: CGColorSpace.sRGB) ?? CGColorSpaceCreateDeviceRGB()
        guard let ctx = CGContext(
            data: &bytes,
            width: w,
            height: h,
            bitsPerComponent: 8,
            bytesPerRow: w * 4,
            space: colorSpace,
            bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue
        ) else {
            return nil
        }
        ctx.draw(image, in: CGRect(x: 0, y: 0, width: w, height: h))
        var pixels: [(r: UInt8, g: UInt8, b: UInt8, a: UInt8)] = []
        pixels.reserveCapacity(w * h)
        for i in stride(from: 0, to: bytes.count, by: 4) {
            pixels.append((bytes[i], bytes[i + 1], bytes[i + 2], bytes[i + 3]))
        }
        return pixels
    }

    // MARK: - OfficeImageLoader 视口虚拟化与按需缓存测试

    @Test("OfficeImageLoader 基础请求、异步解码与回调触发")
    func imageLoaderBasicRequestAndDecode() async throws {
        let pptxURL = samplePath("sample.pptx")
        guard FileManager.default.fileExists(atPath: pptxURL.path) else { return }

        final class CallbackTracker: @unchecked Sendable {
            var count = 0
            let lock = NSLock()
            func increment() {
                lock.lock()
                defer { lock.unlock() }
                count += 1
            }
            var value: Int {
                lock.lock()
                defer { lock.unlock() }
                return count
            }
        }

        let tracker = CallbackTracker()
        let loader = OfficeImageLoader(
            filePath: pptxURL.path,
            maxTotalPixels: 10_000,
            maxCachedImages: 5,
            onImageDecoded: {
                tracker.increment()
            }
        )
        defer { loader.close() }

        let targetPart = "ppt/showcase/base/media/pixel.png"
        loader.request(visibleParts: [targetPart], prefetchParts: [])

        let deadline = Date().addingTimeInterval(5.0)
        var loadedImage: CGImage?
        while Date() < deadline {
            if let img = loader.image(for: targetPart) {
                loadedImage = img
                break
            }
            try await Task.sleep(nanoseconds: 20_000_000)
        }
        if loadedImage == nil {
            loadedImage = loader.image(for: targetPart)
        }

        #expect(loadedImage != nil)
        #expect(loadedImage?.width == 1)
        #expect(loadedImage?.height == 1)
        #expect(loader.cachedCount >= 1)
        #expect(loader.currentTotalPixels >= 1)
        #expect(loader.snapshot[targetPart] != nil)

        // 等待主线程回调派发
        let callbackDeadline = Date().addingTimeInterval(1.0)
        while Date() < callbackDeadline && tracker.value == 0 {
            try await Task.sleep(nanoseconds: 10_000_000)
        }
        #expect(tracker.value >= 1)
    }

    @Test("OfficeImageLoader 空部件与无效部件容错")
    func imageLoaderFaultTolerance() async throws {
        let pptxURL = samplePath("sample.pptx")
        guard FileManager.default.fileExists(atPath: pptxURL.path) else { return }

        let loader = OfficeImageLoader(filePath: pptxURL.path)
        defer { loader.close() }

        // 请求空部件和无效部件
        loader.request(visibleParts: ["", "ppt/media/non_existent.png"], prefetchParts: [""])

        try await Task.sleep(nanoseconds: 80_000_000)

        #expect(loader.image(for: "") == nil)
        #expect(loader.image(for: "ppt/media/non_existent.png") == nil)
        #expect(loader.cachedCount == 0)
        #expect(loader.failedCount >= 1)

        // 验证后续有效请求仍可正常工作
        let validPart = "ppt/showcase/base/media/pixel.png"
        loader.request(visibleParts: [validPart], prefetchParts: [])
        let deadline = Date().addingTimeInterval(3.0)
        var validImg: CGImage?
        while Date() < deadline {
            if let img = loader.image(for: validPart) {
                validImg = img
                break
            }
            try await Task.sleep(nanoseconds: 20_000_000)
        }
        #expect(validImg != nil)
        #expect(loader.cachedCount == 1)
    }

    @Test("OfficeImageLoader 缓存上限与 LRU 淘汰：非 wanted 图片优先淘汰且 wanted 图片受保护")
    func imageLoaderLRUEvictionAndWantedProtection() throws {
        let pptxURL = samplePath("sample.pptx")
        let loader = OfficeImageLoader(
            filePath: pptxURL.path,
            maxTotalPixels: 10_000,
            maxCachedImages: 3
        )
        defer { loader.close() }

        let imgA = createTestCGImage(width: 10, height: 10) // 100 px
        let imgB = createTestCGImage(width: 10, height: 10) // 100 px
        let imgC = createTestCGImage(width: 10, height: 10) // 100 px
        let imgD = createTestCGImage(width: 10, height: 10) // 100 px

        // 1. 模拟视口请求 A 与 B
        loader.request(visibleParts: ["imgA", "imgB"], prefetchParts: [])
        loader.insertImageForTesting(part: "imgA", image: imgA)
        loader.insertImageForTesting(part: "imgB", image: imgB)
        #expect(loader.cachedCount == 2)
        #expect(loader.currentTotalPixels == 200)

        // 2. 插入 C（未超过上限 3）
        loader.insertImageForTesting(part: "imgC", image: imgC)
        #expect(loader.cachedCount == 3)

        // 3. 访问 imgA，使其在 LRU 队列中更新为最新
        #expect(loader.image(for: "imgA") != nil)
        // 此时 accessOrder 为 [imgB, imgC, imgA]

        // 4. 用户滚动：视口变为 [imgA, imgD]，此时 imgB 和 imgC 不在 wanted 中
        loader.request(visibleParts: ["imgA", "imgD"], prefetchParts: [])

        // 5. 插入 imgD，导致缓存达到 4 张，超过 maxCachedImages: 3
        // 淘汰候选人必须是不在 wanted 中的图片，accessOrder 中排在前面的是 imgB
        loader.insertImageForTesting(part: "imgD", image: imgD)

        #expect(loader.cachedCount == 3)
        #expect(loader.image(for: "imgB") == nil, "imgB 最久未访问且不在 wanted，应当被淘汰")
        #expect(loader.image(for: "imgC") != nil, "imgC 比 imgB 新，暂不淘汰")
        #expect(loader.image(for: "imgA") != nil, "imgA 在 wanted 中，应当受保护")
        #expect(loader.image(for: "imgD") != nil, "imgD 刚插入且在 wanted 中，应当存在")

        // 6. 再次插入一张新图 imgE，此时 non-wanted 中只剩下 imgC
        let imgE = createTestCGImage(width: 10, height: 10)
        loader.insertImageForTesting(part: "imgE", image: imgE)
        #expect(loader.cachedCount == 3)
        #expect(loader.image(for: "imgC") == nil, "imgC 作为剩余的 non-wanted 最老项被淘汰")
        #expect(loader.image(for: "imgA") != nil, "imgA 依然受保护")
        #expect(loader.image(for: "imgD") != nil, "imgD 依然受保护")
    }

    @Test("OfficeImageLoader 像素预算超额淘汰与降至预算范围校验")
    func imageLoaderPixelBudgetEviction() throws {
        let pptxURL = samplePath("sample.pptx")
        // 总像素预算 250 px
        let loader = OfficeImageLoader(
            filePath: pptxURL.path,
            maxTotalPixels: 250,
            maxCachedImages: 10
        )
        defer { loader.close() }

        let small1 = createTestCGImage(width: 10, height: 10) // 100 px
        let small2 = createTestCGImage(width: 10, height: 10) // 100 px
        let big = createTestCGImage(width: 15, height: 15)    // 225 px

        // 初始视口包含 small1 和 small2
        loader.request(visibleParts: ["small1", "small2"], prefetchParts: [])
        loader.insertImageForTesting(part: "small1", image: small1)
        loader.insertImageForTesting(part: "small2", image: small2)
        #expect(loader.currentTotalPixels == 200)
        #expect(loader.cachedCount == 2)

        // 视口移动到 big，small1 和 small2 脱离视口
        loader.request(visibleParts: ["big"], prefetchParts: [])

        // 插入 big（225 px），若不淘汰总像素将达到 425 px > 250 px
        loader.insertImageForTesting(part: "big", image: big)

        // 校验淘汰后总像素必须降至预算范围内 (225 <= 250)
        #expect(loader.currentTotalPixels <= 250)
        #expect(loader.currentTotalPixels == 225)
        #expect(loader.image(for: "small1") == nil, "small1 应当已被淘汰以腾出像素预算")
        #expect(loader.image(for: "small2") == nil, "small2 应当已被淘汰以腾出像素预算")
        #expect(loader.image(for: "big") != nil, "big 作为当前视口所需图片被保留")

        // 验证保护机制：若所有图片都在 wanted 中，即使超预算也不会被 trimLocked 强行淘汰
        let extra = createTestCGImage(width: 10, height: 10) // 100 px
        loader.request(visibleParts: ["big", "extra"], prefetchParts: [])
        loader.insertImageForTesting(part: "extra", image: extra)
        #expect(loader.cachedCount == 2)
        #expect(loader.image(for: "big") != nil)
        #expect(loader.image(for: "extra") != nil)
    }

    @Test("OfficeImageLoader 关闭与并发取消安全性")
    func imageLoaderCloseAndCancellationSafety() async throws {
        let pptxURL = samplePath("sample.pptx")
        guard FileManager.default.fileExists(atPath: pptxURL.path) else { return }

        let loader = OfficeImageLoader(filePath: pptxURL.path)

        // 发起批量加载
        loader.request(
            visibleParts: [
                "ppt/showcase/base/media/pixel.png",
                "ppt/showcase/compat/media/pixel.png"
            ],
            prefetchParts: [
                "ppt/showcase/charts/media/pixel.png",
                "ppt/showcase/colors/media/pixel.png"
            ]
        )

        // 立即关闭
        loader.close()

        #expect(loader.isClosedState)
        #expect(loader.cachedCount == 0)
        #expect(loader.currentTotalPixels == 0)
        #expect(loader.snapshot.isEmpty)

        // 等待后台任何可能在跑的任务完成
        try await Task.sleep(nanoseconds: 50_000_000)

        // 关闭后读取必须返回 nil
        #expect(loader.image(for: "ppt/showcase/base/media/pixel.png") == nil)

        // 再次调用 close 幂等且安全
        loader.close()
        #expect(loader.isClosedState)

        // 关闭后调用 request 必须直接忽略
        loader.request(visibleParts: ["ppt/showcase/base/media/pixel.png"], prefetchParts: [])
        #expect(loader.currentWanted.isEmpty)
        #expect(loader.cachedCount == 0)
    }

    @Test("CGElementDrawer 字体解析与排版修饰属性（删除线、上下标、字间距、字重）")
    func fontResolutionAndTypographyAttributes() throws {
        // 1. 字体族回退与解析
        let timesFont = CGElementDrawer.resolveNSFont(face: "Times New Roman", size: 14, bold: false, italic: false)
        #expect(timesFont.familyName == "Times New Roman")

        let simsunFont = CGElementDrawer.resolveNSFont(face: "SimSun", size: 14, bold: false, italic: false)
        #expect(simsunFont.familyName == "Songti SC" || simsunFont.familyName == "STSong" || simsunFont.familyName == "Times New Roman" || simsunFont.familyName == "Georgia")

        let dengxianFont = CGElementDrawer.resolveNSFont(face: "DengXian", size: 14, bold: false, italic: false)
        #expect(dengxianFont.familyName == "PingFang SC" || dengxianFont.familyName == "DengXian")

        let consolasFont = CGElementDrawer.resolveNSFont(face: "Consolas", size: 14, bold: false, italic: false)
        #expect(consolasFont.isFixedPitch || consolasFont.fontDescriptor.symbolicTraits.contains(.monoSpace))

        let kaitiFont = CGElementDrawer.resolveNSFont(face: "KaiTi", size: 14, bold: false, italic: false)
        #expect(kaitiFont.familyName == "Kaiti SC" || kaitiFont.familyName == "STKaiti" || kaitiFont.familyName == "Comic Sans MS")

        // 2. 线程安全缓存命中（同一请求返回相同实例）
        let fontA = CGElementDrawer.resolveNSFont(face: "Times New Roman", size: 16, bold: true, italic: false, weight: 700)
        let fontB = CGElementDrawer.resolveNSFont(face: "Times New Roman", size: 16, bold: true, italic: false, weight: 700)
        #expect(fontA === fontB)

        // 3. OfficeRun 与段落属性构建（删除线、上下标、字间距、字号缩放）
        let paraJson = """
        {
            "runs": [
                {"text": "正常文本", "size": 20, "bold": true, "strike": true, "color": 4294901760},
                {"text": "上标", "size": 20, "vertAlign": "superscript"},
                {"text": "下标", "size": 20, "vertAlign": "subscript"},
                {"text": "间距文本", "size": 20, "spacing": 2.5},
                {"text": "字重文本", "size": 20, "weight": 800}
            ]
        }
        """.data(using: .utf8)!
        let para = try JSONDecoder().decode(OfficeParagraph.self, from: paraJson)
        #expect(para.runs.count == 5)
        #expect(para.runs[0].strike == true)
        #expect(para.runs[1].vertAlign == "superscript")
        #expect(para.runs[2].vertAlign == "subscript")
        #expect(para.runs[3].spacing == 2.5)
        #expect(para.runs[4].weight == 800)

        let attr = CGElementDrawer.buildAttributedString(for: para)
        let str = attr.string as NSString

        let r0 = str.range(of: "正常文本")
        #expect(r0.location != NSNotFound)
        let strikeVal = attr.attribute(.strikethroughStyle, at: r0.location, effectiveRange: nil) as? Int
        #expect(strikeVal == NSUnderlineStyle.single.rawValue)
        let strikeColor = attr.attribute(.strikethroughColor, at: r0.location, effectiveRange: nil) as? NSColor
        #expect(strikeColor != nil)

        let r1 = str.range(of: "上标")
        #expect(r1.location != NSNotFound)
        let superFont = attr.attribute(.font, at: r1.location, effectiveRange: nil) as? NSFont
        #expect(abs((superFont?.pointSize ?? 0) - 13.0) < 0.1) // 20 * 0.65 = 13.0
        let superOffset = attr.attribute(.baselineOffset, at: r1.location, effectiveRange: nil) as? CGFloat
        #expect(abs((superOffset ?? 0) - (13.0 * 0.45)) < 0.1)

        let r2 = str.range(of: "下标")
        #expect(r2.location != NSNotFound)
        let subFont = attr.attribute(.font, at: r2.location, effectiveRange: nil) as? NSFont
        #expect(abs((subFont?.pointSize ?? 0) - 13.0) < 0.1) // 20 * 0.65 = 13.0
        let subOffset = attr.attribute(.baselineOffset, at: r2.location, effectiveRange: nil) as? CGFloat
        #expect(abs((subOffset ?? 0) - (-13.0 * 0.25)) < 0.1)

        let r3 = str.range(of: "间距文本")
        #expect(r3.location != NSNotFound)
        let kernVal = attr.attribute(.kern, at: r3.location, effectiveRange: nil) as? CGFloat
        #expect(abs((kernVal ?? 0) - 2.5) < 0.01)

        let r4 = str.range(of: "字重文本")
        #expect(r4.location != NSNotFound)
        let weightFont = attr.attribute(.font, at: r4.location, effectiveRange: nil) as? NSFont
        #expect(weightFont != nil)

        // 4. OfficeCellStyle 解码与属性完整性校验
        let cellStyleJson = """
        {
            "fontSize": 14,
            "bold": true,
            "italic": false,
            "underline": true,
            "color": 4278190080,
            "fill": 0,
            "alignment": 1,
            "verticalAlignment": "CENTER",
            "wrap": false,
            "borders": [],
            "fontFace": "Consolas",
            "strike": true,
            "vertAlign": "superscript",
            "indent": 2,
            "shrinkToFit": true
        }
        """.data(using: .utf8)!
        let decodedStyle = try JSONDecoder().decode(OfficeCellStyle.self, from: cellStyleJson)
        #expect(decodedStyle.fontFace == "Consolas")
        #expect(decodedStyle.strike == true)
        #expect(decodedStyle.vertAlign == "superscript")
        #expect(decodedStyle.indent == 2)
        #expect(decodedStyle.shrinkToFit == true)
        #expect(decodedStyle.indentWidth(characterWidth: 4, resolvedAlignment: 0) == 8)
        #expect(decodedStyle.indentWidth(characterWidth: 4, resolvedAlignment: 2) == 8)
        #expect(decodedStyle.indentWidth(characterWidth: 4, resolvedAlignment: 1) == 0)
        #expect(decodedStyle.fittedFontSize(baseSize: 14, desiredWidth: 200, availableWidth: 100) == 7)
        #expect(decodedStyle.fittedFontSize(baseSize: 14, desiredWidth: 50, availableWidth: 100) == 14)

        // 5. 缺省字段时 OfficeCellStyle 回退默认值
        let legacyStyleJson = """
        {
            "fontSize": 12,
            "bold": false,
            "italic": false,
            "underline": false,
            "color": 4278190080,
            "fill": 0,
            "alignment": 0,
            "verticalAlignment": "TOP",
            "wrap": false,
            "borders": []
        }
        """.data(using: .utf8)!
        let defaultStyle = try JSONDecoder().decode(OfficeCellStyle.self, from: legacyStyleJson)
        #expect(defaultStyle.fontFace == "")
        #expect(defaultStyle.strike == false)
        #expect(defaultStyle.vertAlign == "")
        #expect(defaultStyle.indent == 0)
        #expect(defaultStyle.shrinkToFit == false)
        #expect(defaultStyle.fittedFontSize(baseSize: 12, desiredWidth: 200, availableWidth: 100) == 12)

        // 6. 电子表格单元格样式包含 fontFace、strike 与 vertAlign 时绘制无崩溃且生成有效图像
        let sheetJson = """
        {
            "name": "Sheet1",
            "rowHeights": [30],
            "columnWidths": [120],
            "cells": [
                {"row": 0, "column": 0, "text": "H2O", "style": 0, "numeric": false}
            ]
        }
        """.data(using: .utf8)!
        let testSheet = try JSONDecoder().decode(OfficeSheet.self, from: sheetJson)
        let cellStyle = OfficeCellStyle(
            fontSize: 12,
            bold: false,
            italic: false,
            underline: true,
            color: 0xff000000,
            fontFace: "Times New Roman",
            strike: true,
            vertAlign: "superscript"
        )
        let colorSpace = CGColorSpace(name: CGColorSpace.sRGB) ?? CGColorSpaceCreateDeviceRGB()
        var bytes = [UInt8](repeating: 255, count: 200 * 100 * 4)
        let ctx = CGContext(
            data: &bytes,
            width: 200,
            height: 100,
            bitsPerComponent: 8,
            bytesPerRow: 200 * 4,
            space: colorSpace,
            bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue
        )!
        CGElementDrawer.drawSpreadsheet(in: ctx, sheet: testSheet, styles: [cellStyle], viewport: CGRect(x: 0, y: 0, width: 200, height: 100))
        let img = ctx.makeImage()!
        #expect(hasNonWhitePixel(img))
    }

    @Test("字体缓存按 LRU 顺序淘汰且容量受限")
    func fontCacheCapacityAndEviction() {
        var cache = CGElementDrawer.FontLRUCache(capacity: 2)
        let first = NSFont.systemFont(ofSize: 10)
        let second = NSFont.systemFont(ofSize: 11)
        let third = NSFont.systemFont(ofSize: 12)
        let fourth = NSFont.systemFont(ofSize: 13)

        #expect(cache.insert(first, forKey: "first") === first)
        #expect(cache.insert(second, forKey: "second") === second)
        #expect(cache.value(forKey: "first") === first)

        // first 最近被使用，插入 third 时应淘汰 second。
        #expect(cache.insert(third, forKey: "third") === third)
        #expect(cache.count == 2)
        #expect(cache.value(forKey: "second") == nil)
        #expect(cache.value(forKey: "first") === first)
        #expect(cache.value(forKey: "third") === third)

        // 重复键保留原实例并刷新使用顺序，下一次插入淘汰 third。
        #expect(cache.insert(NSFont.systemFont(ofSize: 14), forKey: "first") === first)
        #expect(cache.insert(fourth, forKey: "fourth") === fourth)
        #expect(cache.count == 2)
        #expect(cache.value(forKey: "third") == nil)
        #expect(cache.value(forKey: "first") === first)
        #expect(cache.value(forKey: "fourth") === fourth)
    }

    @Test("段落底纹、高亮与边框解码及渲染属性校验")
    func testParagraphShadingBordersAndHighlighting() throws {
        let json = """
        {
            "schemaVersion": 9,
            "kind": "DOCX",
            "width": 595,
            "blocks": [
                {
                    "type": "TEXT",
                    "width": 500,
                    "height": 100,
                    "paragraphs": [
                        {
                            "background": 4294901760,
                            "borderTop": 4278190080,
                            "borderBottom": 4278190080,
                            "borderLeft": 4278190080,
                            "borderRight": 4278190080,
                            "borderWidth": 1.5,
                            "runs": [
                                {
                                    "text": "高亮文本",
                                    "size": 16,
                                    "color": 4278190080,
                                    "background": 4294967040
                                }
                            ]
                        }
                    ]
                }
            ]
        }
        """.data(using: .utf8)!

        let doc = try JSONDecoder().decode(OfficeDocument.self, from: json)
        #expect(doc.blocks.count == 1)
        let block = doc.blocks[0]
        #expect(block.paragraphs.count == 1)
        let para = block.paragraphs[0]
        #expect(para.background == 4294901760)
        #expect(para.borderTop == 4278190080)
        #expect(para.borderBottom == 4278190080)
        #expect(para.borderLeft == 4278190080)
        #expect(para.borderRight == 4278190080)
        #expect(para.borderWidth == 1.5)
        #expect(para.runs.count == 1)
        let run = para.runs[0]
        #expect(run.text == "高亮文本")
        #expect(run.background == 4294967040)

        // 验证 buildAttributedString 挂载了 .backgroundColor
        let attr = CGElementDrawer.buildAttributedString(for: para)
        #expect(attr.length > 0)
        let bg = attr.attribute(.backgroundColor, at: 0, effectiveRange: nil) as? NSColor
        #expect(bg != nil)

        // 验证 measureParagraphsHeight 与离屏渲染
        let measuredH = CGElementDrawer.measureParagraphsHeight(block.paragraphs, availableWidth: 500, wrap: true)
        #expect(measuredH > 0)

        // 验证绘制包含段落底纹、边框和高亮
        let colorSpace = CGColorSpace(name: CGColorSpace.sRGB) ?? CGColorSpaceCreateDeviceRGB()
        var bytes = [UInt8](repeating: 255, count: 500 * 200 * 4)
        let ctx = CGContext(
            data: &bytes,
            width: 500,
            height: 200,
            bitsPerComponent: 8,
            bytesPerRow: 500 * 4,
            space: colorSpace,
            bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue
        )!
        CGElementDrawer.drawElement(in: ctx, element: block, images: [:])
        let img = ctx.makeImage()!
        #expect(hasNonWhitePixel(img))
    }

    @Test("DOCX 浮动环绕字段解码与默认值兼容")
    func docxFloatingWrapDecodingAndDefaults() throws {
        let json = """
        {
          "schemaVersion": 9,
          "kind": "DOCX",
          "width": 595,
          "blocks": [
            {
              "type": "TEXT",
              "paragraphs": [{ "runs": [{ "text": "Plain inline", "size": 12 }] }]
            },
            {
              "type": "IMAGE",
              "x": 120,
              "y": 16,
              "width": 100,
              "height": 80,
              "image": "word/media/image1.png",
              "wrapMode": "SQUARE",
              "horizontalAlign": 3,
              "behindDoc": true,
              "distTop": 4,
              "distBottom": 6,
              "distLeft": 10,
              "distRight": 12
            },
            {
              "type": "RECT",
              "width": 80,
              "height": 40,
              "wrapMode": "TOP_AND_BOTTOM",
              "horizontalAlign": 99,
              "distTop": -5,
              "distBottom": 2500
            }
          ]
        }
        """.data(using: .utf8)!

        let doc = try JSONDecoder().decode(OfficeDocument.self, from: json)
        #expect(doc.blocks.count == 3)

        let inlineEl = doc.blocks[0]
        #expect(inlineEl.wrapMode == .inline)
        #expect(inlineEl.horizontalAlign == 0)
        #expect(inlineEl.behindDoc == false)
        #expect(inlineEl.distTop == 0)
        #expect(inlineEl.distBottom == 0)
        #expect(inlineEl.distLeft == 0)
        #expect(inlineEl.distRight == 0)

        let floatImg = doc.blocks[1]
        #expect(floatImg.wrapMode == .square)
        #expect(floatImg.horizontalAlign == 3)
        #expect(floatImg.behindDoc == true)
        #expect(floatImg.distTop == 4)
        #expect(floatImg.distBottom == 6)
        #expect(floatImg.distLeft == 10)
        #expect(floatImg.distRight == 12)

        let clampedEl = doc.blocks[2]
        #expect(clampedEl.wrapMode == .topAndBottom)
        #expect(clampedEl.horizontalAlign == 0)
        #expect(clampedEl.distTop == 0)
        #expect(clampedEl.distBottom == 1200)
    }

    @Test("DOCX 浮动布局、Z-Order、文字环绕高亮与固定尺寸 Shape 渲染")
    func docxFloatingLayoutAndShapeRendering() throws {
        let json = """
        {
          "schemaVersion": 9,
          "kind": "DOCX",
          "width": 595,
          "marginLeft": 40,
          "marginRight": 40,
          "marginTop": 40,
          "marginBottom": 40,
          "blocks": [
            {
              "type": "IMAGE",
              "width": 120,
              "height": 90,
              "image": "word/media/float1.png",
              "wrapMode": "SQUARE",
              "horizontalAlign": 1,
              "behindDoc": true,
              "distRight": 10
            },
            {
              "type": "TEXT",
              "paragraphs": [{
                "runs": [{
                  "text": "WrappedBodyText word1 word2 word3 word4 word5 word6 word7 word8 word9 word10 word11 word12 word13 word14 word15 word16 word17 word18 word19 word20 word21 word22 word23 word24 word25 word26 word27 word28",
                  "size": 12
                }]
              }]
            },
            {
              "type": "RECT",
              "width": 200,
              "height": 60,
              "fill": 4280391411,
              "stroke": 4278190080,
              "strokeWidth": 1.5,
              "wrapMode": "TOP_AND_BOTTOM",
              "horizontalAlign": 2,
              "distTop": 6,
              "distBottom": 8
            },
            {
              "type": "TEXT",
              "paragraphs": [{
                "runs": [{ "text": "AfterTopBottom", "size": 12 }]
              }]
            },
            {
              "type": "RECT",
              "x": 300,
              "y": 0,
              "width": 150,
              "height": 70,
              "padding": 8,
              "fill": 4294959296,
              "stroke": 4278190080,
              "strokeWidth": 1.5,
              "verticalAlignment": "CENTER",
              "wrapMode": "NONE",
              "behindDoc": false,
              "paragraphs": [{
                "alignment": 1,
                "runs": [{ "text": "Floating Shape Box", "size": 12, "color": 4278190080 }]
              }]
            }
          ]
        }
        """.data(using: .utf8)!

        let doc = try JSONDecoder().decode(OfficeDocument.self, from: json)
        let plan = CGElementDrawer.layoutDocxBlocks(in: doc)
        #expect(plan.placements.count == 5)
        #expect(plan.zOrder == [0, 1, 3, 2, 4])

        // 左侧浮动图位于左页边距处
        let floatPlacement = plan.placements[0]
        #expect(floatPlacement.frame.minX == 40)
        #expect(floatPlacement.frame.width == 120)
        #expect(floatPlacement.behindDoc == true)

        // 被环绕段落存在左侧障碍物，高度大于无障碍排版
        let wrappedPlacement = plan.placements[1]
        #expect(wrappedPlacement.obstacles.count == 1)
        #expect(wrappedPlacement.obstacles[0].side == .left)
        let availTextW = wrappedPlacement.frame.width - 8
        let wrappedH = CGElementDrawer.measureParagraphsHeight(
            wrappedPlacement.element.paragraphs,
            availableWidth: availTextW,
            wrap: true,
            obstacles: wrappedPlacement.obstacles
        )
        let plainH = CGElementDrawer.measureParagraphsHeight(
            wrappedPlacement.element.paragraphs,
            availableWidth: availTextW,
            wrap: true
        )
        #expect(wrappedH > plainH)

        // 高亮矩形受左侧障碍物右推
        let searchMatch = OfficeSearchMatch(pageIndex: 0, elementIndex: 1, paragraphIndex: 0, startChar: 0, endChar: 15, snippet: "WrappedBodyText")
        let rects = CGElementDrawer.computeHighlightRects(for: searchMatch, in: doc, docxPlan: plan)
        #expect(!rects.isEmpty)
        #expect(rects[0].minX >= 40 + 120 + 10)

        // TOP_AND_BOTTOM 与固定尺寸 shape
        let topBottomPlacement = plan.placements[2]
        let afterPlacement = plan.placements[3]
        #expect(afterPlacement.frame.minY >= topBottomPlacement.frame.maxY + 8)

        let shapePlacement = plan.placements[4]
        #expect(shapePlacement.frame.minX == 300)
        #expect(shapePlacement.frame.width == 150)
        #expect(shapePlacement.frame.height == 70)

        // TextSelectionModel 覆盖浮动与环绕文本
        let textLayout = DocumentTextLayout.build(from: doc)
        if let allRange = textLayout.allTextRange() {
            let selected = textLayout.selectedText(for: allRange)
            #expect(selected.contains("WrappedBodyText"))
            #expect(selected.contains("Floating Shape Box"))
        } else {
            Issue.record("Expected non-nil allTextRange")
        }

        // 离屏绘制所有层
        let colorSpace = CGColorSpace(name: CGColorSpace.sRGB) ?? CGColorSpaceCreateDeviceRGB()
        var bytes = [UInt8](repeating: 255, count: 595 * 400 * 4)
        let ctx = CGContext(
            data: &bytes,
            width: 595,
            height: 400,
            bitsPerComponent: 8,
            bytesPerRow: 595 * 4,
            space: colorSpace,
            bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue
        )!
        let testImg = createTestCGImage(width: 40, height: 30)
        for idx in plan.zOrder {
            CGElementDrawer.drawDocxPlacement(
                in: ctx,
                placement: plan.placements[idx],
                images: ["word/media/float1.png": testImg]
            )
        }
        let img = ctx.makeImage()!
        #expect(hasNonWhitePixel(img))
    }

    @Test("DOCX 内联图片水平对齐、spAutoFit 文本框紧缩居中与预览容器居中计算")
    func docxInlineAlignmentAutofitAndViewportCentering() throws {
        let json = """
        {
          "schemaVersion": 9,
          "kind": "DOCX",
          "width": 595,
          "marginLeft": 90,
          "marginRight": 90,
          "marginTop": 72,
          "marginBottom": 72,
          "blocks": [
            {
              "type": "IMAGE",
              "width": 172,
              "height": 300,
              "image": "word/media/left.png",
              "wrapMode": "INLINE",
              "horizontalAlign": 1
            },
            {
              "type": "IMAGE",
              "width": 172,
              "height": 300,
              "image": "word/media/center.png",
              "wrapMode": "INLINE",
              "horizontalAlign": 2
            },
            {
              "type": "IMAGE",
              "width": 172,
              "height": 300,
              "image": "word/media/right.png",
              "wrapMode": "INLINE",
              "horizontalAlign": 3
            }
          ],
          "footerBlocks": [
            {
              "type": "TEXT",
              "width": 0,
              "height": 0,
              "padding": 0,
              "textWrap": false,
              "wrapMode": "NONE",
              "horizontalAlign": 2,
              "paragraphs": [{
                "alignment": 1,
                "runs": [{ "text": "1", "size": 9 }]
              }]
            }
          ]
        }
        """.data(using: .utf8)!

        let doc = try JSONDecoder().decode(OfficeDocument.self, from: json)
        let plan = CGElementDrawer.layoutDocxBlocks(in: doc)
        #expect(plan.placements.count == 4)

        let availW: CGFloat = 595 - 90 - 90
        let leftFrame = plan.placements[0].frame
        let centerFrame = plan.placements[1].frame
        let rightFrame = plan.placements[2].frame
        let footerFrame = plan.placements[3].frame

        #expect(abs(leftFrame.minX - 90) < 0.5)
        #expect(abs(centerFrame.minX - (90 + (availW - 172) / 2.0)) < 0.5)
        #expect(abs(rightFrame.minX - (90 + availW - 172)) < 0.5)

        // 页脚 spAutoFit + wrap=none 文本框应紧缩至单字符宽度并水平居中于页心
        #expect(footerFrame.width > 0 && footerFrame.width < 24)
        #expect(footerFrame.height > 0 && footerFrame.height < 30)
        #expect(abs(footerFrame.midX - 595.0 / 2.0) < 1.0)

        // 视口居中宽度/高度与画布尺寸辅助：DOCX/PPTX 水平居中且顶对齐保留 16pt 外边距，XLSX 左上对齐铺满视口且无外边距
        #expect(OfficePreviewView.minContainerWidth(for: 920.6, kind: .docx) == 888)
        #expect(OfficePreviewView.minContainerWidth(for: 920.6, kind: .pptx) == 888)
        #expect(OfficePreviewView.minContainerWidth(for: 920.6, kind: .xlsx) == 0)
        #expect(OfficePreviewView.minContainerWidth(for: 16, kind: .docx) == 0)

        #expect(OfficePreviewView.canvasPadding(for: .docx) == 16)
        #expect(OfficePreviewView.canvasPadding(for: .pptx) == 16)
        #expect(OfficePreviewView.canvasPadding(for: .xlsx) == 0)

        #expect(OfficePreviewView.minContainerHeight(for: 640.8, kind: .docx) == 608)
        #expect(OfficePreviewView.minContainerHeight(for: 640.8, kind: .pptx) == 608)
        #expect(OfficePreviewView.minContainerHeight(for: 640.8, kind: .xlsx) == 640)

        #expect(OfficePreviewView.canvasFrameDimension(scaled: 300, viewport: 900.7, kind: .xlsx) == 900)
        #expect(OfficePreviewView.canvasFrameDimension(scaled: 1200, viewport: 900.7, kind: .xlsx) == 1200)
        #expect(OfficePreviewView.canvasFrameDimension(scaled: 300, viewport: 900.7, kind: .docx) == 300)
    }

    // MARK: - XLSX 冻结窗格

    /// 构造带冻结配置的测试工作表 JSON
    private func freezeSheet(frozenRows: Int? = nil, frozenColumns: Int? = nil) throws -> OfficeSheet {
        var fields: [String] = []
        if let frozenRows { fields.append("\"frozenRows\": \(frozenRows)") }
        if let frozenColumns { fields.append("\"frozenColumns\": \(frozenColumns)") }
        let extra = fields.isEmpty ? "" : ",\n    " + fields.joined(separator: ",\n    ")
        let json = """
        {
            "name": "Frozen",
            "rowHeights": [30, 30, 30, 30, 30, 30],
            "columnWidths": [60, 60, 60, 60, 60, 60],
            "cells": [
                {"row": 0, "column": 0, "text": "HEAD", "style": 0, "numeric": false},
                {"row": 5, "column": 5, "text": "TAIL", "style": 0, "numeric": false}
            ],
            "merges": []\(extra)
        }
        """.data(using: .utf8)!
        return try JSONDecoder().decode(OfficeSheet.self, from: json)
    }

    @Test("OfficeSheet 冻结字段解码、缺省与钳位")
    func sheetFrozenFieldsDecoding() throws {
        // 缺省：无冻结字段时为 0，保持与旧 core 的兼容
        let plain = try freezeSheet()
        #expect(plain.frozenRows == 0)
        #expect(plain.frozenColumns == 0)

        let frozen = try freezeSheet(frozenRows: 2, frozenColumns: 3)
        #expect(frozen.frozenRows == 2)
        #expect(frozen.frozenColumns == 3)

        // 超过表维度时钳位到行列数量
        let oversized = try freezeSheet(frozenRows: 99, frozenColumns: 99)
        #expect(oversized.frozenRows == 6)
        #expect(oversized.frozenColumns == 6)

        // 负数钳位到 0
        let negative = try freezeSheet(frozenRows: -2, frozenColumns: -1)
        #expect(negative.frozenRows == 0)
        #expect(negative.frozenColumns == 0)
    }

    @Test("XLSX 浮动元素解码并按冻结四象限平移绘制裁剪")
    func sheetElementsDecodingAndQuadrantRendering() throws {
        // 旧 core 不输出 elements 时必须回退为空数组；新字段沿用通用 Element 解码。
        let plain = try freezeSheet()
        #expect(plain.elements.isEmpty)
        let decoded = try JSONDecoder().decode(OfficeSheet.self, from: """
        {
            "name": "Chart",
            "rowHeights": [30],
            "columnWidths": [60],
            "cells": [],
            "merges": [],
            "elements": [{
                "type": "RECT",
                "x": 10,
                "y": 5,
                "width": 20,
                "height": 10,
                "transform": [1, 0, 0, 1, 10, 5],
                "fill": 4294901760,
                "stroke": 0
            }]
        }
        """.data(using: .utf8)!)
        #expect(decoded.elements.count == 1)
        #expect(decoded.elements[0].type == .rect)
        #expect(decoded.elements[0].x == 10 && decoded.elements[0].y == 5)
        #expect(decoded.elements[0].transform == [1, 0, 0, 1, 10, 5])

        func rectElement(_ x: CGFloat, _ y: CGFloat, _ fill: UInt32) -> OfficeElement {
            OfficeElement(
                type: .rect,
                x: x,
                y: y,
                width: 20,
                height: 10,
                transform: [1, 0, 0, 1, x, y],
                fill: fill,
                stroke: 0
            )
        }
        let sheet = OfficeSheet(
            name: "ChartQuadrants",
            rowHeights: Array(repeating: 30, count: 6),
            columnWidths: Array(repeating: 60, count: 6),
            cells: [],
            merges: [],
            elements: [
                rectElement(10, 5, 0xffff0000),
                rectElement(120, 5, 0xff00ff00),
                rectElement(10, 60, 0xff0000ff),
                rectElement(120, 60, 0xffffff00),
                // 负向 y 翻转线段：校验 bounds 剔除正确处理负缩放矩阵且仍正常绘制。
                OfficeElement(
                    type: .line,
                    x: 150,
                    y: 70,
                    width: 20,
                    height: 10,
                    transform: [1, 0, 0, -1, 150, 80],
                    fill: 0,
                    stroke: 0xff000000,
                    strokeWidth: 4
                ),
                // 象限外元素：不得在视口内绘制。
                rectElement(500, 500, 0xffff00ff)
            ],
            frozenRows: 1,
            frozenColumns: 1
        )

        // 与单元格四象限测试使用同一滚动视口。元素矩阵已经包含数据区 x/y，
        // 绘制层只应补 42/22 gutter 与各象限的冻结平移。
        let viewport = CGRect(x: 60, y: 30, width: 200, height: 160)
        let vpW = 200, vpH = 160
        let colorSpace = CGColorSpace(name: CGColorSpace.sRGB) ?? CGColorSpaceCreateDeviceRGB()
        var bytes = [UInt8](repeating: 255, count: vpW * vpH * 4)
        let ctx = CGContext(
            data: &bytes,
            width: vpW,
            height: vpH,
            bitsPerComponent: 8,
            bytesPerRow: vpW * 4,
            space: colorSpace,
            bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue
        )!
        ctx.translateBy(x: 0, y: CGFloat(vpH))
        ctx.scaleBy(x: 1, y: -1)
        ctx.translateBy(x: -viewport.minX, y: -viewport.minY)
        CGElementDrawer.drawSpreadsheet(in: ctx, sheet: sheet, styles: [], viewport: viewport)

        func pixelRGBA(_ x: Int, _ y: Int) -> (UInt8, UInt8, UInt8, UInt8) {
            let idx = (y * vpW + x) * 4
            return (bytes[idx], bytes[idx + 1], bytes[idx + 2], bytes[idx + 3])
        }
        func matchesRGB(_ p: (UInt8, UInt8, UInt8, UInt8), _ r: Int, _ g: Int, _ b: Int) -> Bool {
            abs(Int(p.0) - r) <= 6 && abs(Int(p.1) - g) <= 6 && abs(Int(p.2) - b) <= 6
        }

        // 交叉角红、冻结行绿、冻结列蓝、滚动区黄，以及负缩放翻转线段中点黑。
        #expect(matchesRGB(pixelRGBA(55, 30), 255, 0, 0))
        #expect(matchesRGB(pixelRGBA(105, 30), 0, 255, 0))
        #expect(matchesRGB(pixelRGBA(55, 55), 0, 0, 255))
        #expect(matchesRGB(pixelRGBA(105, 55), 255, 255, 0))
        #expect(matchesRGB(pixelRGBA(142, 67), 0, 0, 0))
        // 新增元素不得越过正文 clip 覆盖行头与列头。
        #expect(matchesRGB(pixelRGBA(150, 10), 0xf3, 0xf5, 0xf7))
        #expect(matchesRGB(pixelRGBA(20, 100), 0xf3, 0xf5, 0xf7))
    }

    @Test("冻结占比保护按轴独立降级")
    func sheetFreezeRatioProtection() {
        let viewport = CGRect(x: 0, y: 0, width: 400, height: 300)
        // 可用正文区 400-42=358 宽、300-22=278 高
        // 冻结列 300pt > 358*0.75=268.5 → 仅列降级，行仍保持冻结
        let columnHeavy = CGElementDrawer.resolveFreezePlan(
            frozenRows: 2,
            frozenColumns: 2,
            rowCount: 6,
            columnCount: 6,
            frozenDataWidth: 300,
            frozenDataHeight: 60,
            totalWidth: 400,
            totalHeight: 300,
            viewport: viewport
        )
        #expect(columnHeavy.effectiveColumns == 0)
        #expect(columnHeavy.effectiveRows == 2)
        #expect(columnHeavy.frozenDataWidth == 0)
        #expect(columnHeavy.frozenDataHeight == 60)

        // 冻结行 250pt > 278*0.75=208.5 → 仅行降级，列仍保持冻结
        let rowHeavy = CGElementDrawer.resolveFreezePlan(
            frozenRows: 2,
            frozenColumns: 2,
            rowCount: 6,
            columnCount: 6,
            frozenDataWidth: 60,
            frozenDataHeight: 250,
            totalWidth: 400,
            totalHeight: 300,
            viewport: viewport
        )
        #expect(rowHeavy.effectiveRows == 0)
        #expect(rowHeavy.effectiveColumns == 2)

        // 恰好等于 75% 边界时不降级
        let boundary = CGElementDrawer.resolveFreezePlan(
            frozenRows: 1,
            frozenColumns: 1,
            rowCount: 6,
            columnCount: 6,
            frozenDataWidth: 358 * 0.75,
            frozenDataHeight: 278 * 0.75,
            totalWidth: 400,
            totalHeight: 300,
            viewport: viewport
        )
        #expect(boundary.effectiveColumns == 1)
        #expect(boundary.effectiveRows == 1)

        // 可用正文区非正时全部降级
        let tiny = CGElementDrawer.resolveFreezePlan(
            frozenRows: 1,
            frozenColumns: 1,
            rowCount: 6,
            columnCount: 6,
            frozenDataWidth: 10,
            frozenDataHeight: 10,
            totalWidth: 40,
            totalHeight: 20,
            viewport: CGRect(x: 0, y: 0, width: 42, height: 22)
        )
        #expect(tiny.effectiveRows == 0)
        #expect(tiny.effectiveColumns == 0)
    }

    @Test("冻结窗格四象限切分与离屏滚动绘制")
    func sheetFreezeQuadrantsAndRender() throws {
        let sheet = try freezeSheet(frozenRows: 1, frozenColumns: 1)
        // 滚动视口：已向右下滚动 60pt，视口 200x160
        let viewport = CGRect(x: 60, y: 30, width: 200, height: 160)
        let plan = CGElementDrawer.sheetFreezePlan(sheet: sheet, viewport: viewport)
        #expect(plan.effectiveRows == 1)
        #expect(plan.effectiveColumns == 1)
        #expect(plan.scrollX == 60)
        #expect(plan.scrollY == 30)
        // 冻结起点紧贴表头右侧/下侧，表头锚定在视口左上角 (scrollX, scrollY)
        #expect(plan.headerLeft == 60)
        #expect(plan.headerTop == 30)
        #expect(plan.frozenOriginX == CGFloat(42 + 60))
        #expect(plan.frozenOriginY == CGFloat(22 + 30))
        #expect(plan.frozenDataWidth == 60)
        #expect(plan.frozenDataHeight == 30)

        let quadrants = CGElementDrawer.sheetQuadrants(plan: plan, rowCount: 6, columnCount: 6)
        #expect(quadrants.count == 4)
        // 象限互不重叠：滚动正文区从 (frozenRight, frozenBottom) 开始
        let scrolling = quadrants[0]
        #expect(scrolling.rowRange == 1..<6)
        #expect(scrolling.columnRange == 1..<6)
        #expect(scrolling.clip.minX == plan.frozenRight)
        #expect(scrolling.clip.minY == plan.frozenBottom)
        // 交叉角锁定双轴
        let corner = quadrants[3]
        #expect(corner.rowRange == 0..<1)
        #expect(corner.columnRange == 0..<1)
        #expect(corner.clip.minX == plan.frozenOriginX)
        #expect(corner.clip.minY == plan.frozenOriginY)

        // 未冻结时只有一个滚动象限，行为与改造前一致
        let plainPlan = CGElementDrawer.sheetFreezePlan(sheet: try freezeSheet(), viewport: viewport)
        #expect(CGElementDrawer.sheetQuadrants(plan: plainPlan, rowCount: 6, columnCount: 6).count == 1)
        #expect(plainPlan.hasFrozen == false)

        // 离屏绘制：带滚动视口也必须产出非空白像素
        let colorSpace = CGColorSpace(name: CGColorSpace.sRGB) ?? CGColorSpaceCreateDeviceRGB()
        let width = 300, height = 240
        var bytes = [UInt8](repeating: 255, count: width * height * 4)
        let ctx = CGContext(
            data: &bytes,
            width: width,
            height: height,
            bitsPerComponent: 8,
            bytesPerRow: width * 4,
            space: colorSpace,
            bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue
        )!
        CGElementDrawer.drawSpreadsheet(in: ctx, sheet: sheet, styles: [], viewport: viewport)
        let image = ctx.makeImage()!
        #expect(hasNonWhitePixel(image))

        // 彩色四象限离屏像素验证（翻转坐标以匹配 NSView.isFlipped），确认表头固定在视口边缘而不遮挡冻结行/列
        let colorJson = """
        {
            "name": "ColoredFreeze",
            "rowHeights": [30, 30, 30, 30, 30, 30],
            "columnWidths": [60, 60, 60, 60, 60, 60],
            "frozenRows": 1,
            "frozenColumns": 1,
            "cells": [
                {"row": 0, "column": 0, "text": "", "style": 0, "numeric": false},
                {"row": 0, "column": 2, "text": "", "style": 1, "numeric": false},
                {"row": 2, "column": 0, "text": "", "style": 2, "numeric": false},
                {"row": 2, "column": 2, "text": "", "style": 3, "numeric": false}
            ],
            "merges": []
        }
        """.data(using: .utf8)!
        let colorSheet = try JSONDecoder().decode(OfficeSheet.self, from: colorJson)
        let quadStyles: [OfficeCellStyle] = [0xffff0000, 0xff00ff00, 0xff0000ff, 0xffffff00].map {
            OfficeCellStyle(fill: $0)
        }
        let vpW = 200, vpH = 160
        var vpBytes = [UInt8](repeating: 255, count: vpW * vpH * 4)
        let vpCtx = CGContext(
            data: &vpBytes,
            width: vpW,
            height: vpH,
            bitsPerComponent: 8,
            bytesPerRow: vpW * 4,
            space: colorSpace,
            bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue
        )!
        vpCtx.translateBy(x: 0, y: CGFloat(vpH))
        vpCtx.scaleBy(x: 1, y: -1)
        vpCtx.translateBy(x: -viewport.minX, y: -viewport.minY)
        CGElementDrawer.drawSpreadsheet(in: vpCtx, sheet: colorSheet, styles: quadStyles, viewport: viewport)

        func pixelRGBA(_ x: Int, _ y: Int) -> (UInt8, UInt8, UInt8, UInt8) {
            let idx = (y * vpW + x) * 4
            return (vpBytes[idx], vpBytes[idx + 1], vpBytes[idx + 2], vpBytes[idx + 3])
        }
        func matchesRGB(_ p: (UInt8, UInt8, UInt8, UInt8), _ r: Int, _ g: Int, _ b: Int, tol: Int = 6) -> Bool {
            abs(Int(p.0) - r) <= tol && abs(Int(p.1) - g) <= tol && abs(Int(p.2) - b) <= tol
        }

        // 顶部列头与左侧行头背景色 #f3f5f7，紧贴视口顶部/左侧而不覆盖第 0 行/第 0 列
        #expect(matchesRGB(pixelRGBA(150, 10), 0xf3, 0xf5, 0xf7))
        #expect(matchesRGB(pixelRGBA(20, 100), 0xf3, 0xf5, 0xf7))
        // 四象限正文像素：交叉角红、冻结行绿、冻结列蓝、滚动区黄
        #expect(matchesRGB(pixelRGBA(72, 37), 255, 0, 0))
        #expect(matchesRGB(pixelRGBA(132, 37), 0, 255, 0))
        #expect(matchesRGB(pixelRGBA(72, 67), 0, 0, 255))
        #expect(matchesRGB(pixelRGBA(132, 67), 255, 255, 0))
        // 冻结交界线 (#9aa5b1，1px 抗锯齿跨像素混合后差值约 50，用 tol: 60 区分普通网格线与白底)
        let hasVDivider = (101...103).contains { matchesRGB(pixelRGBA($0, 100), 0x9a, 0xa5, 0xb1, tol: 60) }
        let hasHDivider = (51...53).contains { matchesRGB(pixelRGBA(132, $0), 0x9a, 0xa5, 0xb1, tol: 60) }
        #expect(hasVDivider)
        #expect(hasHDivider)
    }

    @Test("检索高亮按象限裁剪且跳转按轴避让")
    func sheetHighlightClipAndScrollTarget() throws {
        let sheet = try freezeSheet(frozenRows: 1, frozenColumns: 1)
        let viewport = CGRect(x: 60, y: 30, width: 200, height: 160)
        let plan = CGElementDrawer.sheetFreezePlan(sheet: sheet, viewport: viewport)

        // 滚动区内的单元格：平移后只落在滚动正文象限，绝不覆盖冻结区
        let scrollingCell = CGRect(x: 200, y: 100, width: 60, height: 30)
        let scrollingPieces = CGElementDrawer.clipSheetHighlight(scrollingCell, plan: plan, rowCount: 6, columnCount: 6)
        #expect(scrollingPieces.count == 1)
        #expect(scrollingPieces[0].clip.minX == plan.frozenRight)
        #expect(scrollingPieces[0].clip.minY == plan.frozenBottom)

        // 交叉角内的单元格：锁定双轴，落在交叉角象限内
        let cornerCell = CGRect(x: 42, y: 22, width: 60, height: 30)
        let cornerPieces = CGElementDrawer.clipSheetHighlight(cornerCell, plan: plan, rowCount: 6, columnCount: 6)
        #expect(cornerPieces.count == 1)
        #expect(cornerPieces[0].clip.minX == plan.frozenOriginX)
        #expect(cornerPieces[0].clip.minY == plan.frozenOriginY)

        // 冻结列内的单元格：保持当前水平滚动偏移不变
        let frozenColumnCell = CGRect(x: 42, y: 150, width: 60, height: 30)
        let frozenTarget = CGElementDrawer.sheetScrollTarget(
            docRect: frozenColumnCell,
            plan: plan,
            clipSize: CGSize(width: 200, height: 160),
            contentOffset: CGPoint(x: 60, y: 30),
            contentSize: CGSize(width: 500, height: 500),
            zoom: 1
        )
        #expect(frozenTarget.x == 60)

        // 可滚动列内的单元格：居中对齐到可滚动视口中心
        let scrollTarget = CGElementDrawer.sheetScrollTarget(
            docRect: scrollingCell,
            plan: plan,
            clipSize: CGSize(width: 200, height: 160),
            contentOffset: CGPoint(x: 60, y: 30),
            contentSize: CGSize(width: 500, height: 500),
            zoom: 1
        )
        #expect(scrollTarget.x != 60)
        #expect(scrollTarget.x >= 0)
        #expect(scrollTarget.x <= 300)

        // 缩放倍数 != 1 时按 zoom 换算像素冻结宽度，命中目标仍对齐至可滚动视口中心
        let zoomedTarget = CGElementDrawer.sheetScrollTarget(
            docRect: scrollingCell,
            plan: plan,
            clipSize: CGSize(width: 200, height: 160),
            contentOffset: CGPoint(x: 60, y: 30),
            contentSize: CGSize(width: 1000, height: 1000),
            zoom: 2
        )
        #expect(abs(zoomedTarget.x - 258) < 0.5)
        #expect(abs(zoomedTarget.y - 98) < 0.5)

        // 滚离原点时，原始文档矩形已不与视口相交，但冻结行/列内的高亮仍经象限映射保留可见
        let farViewport = CGRect(x: 180, y: 90, width: 200, height: 160)
        let farPlan = CGElementDrawer.sheetFreezePlan(sheet: sheet, viewport: farViewport)
        let frozenRowMatch = CGRect(x: 240, y: 22, width: 60, height: 30)
        #expect(!frozenRowMatch.intersects(farViewport))
        let frozenRowPieces = CGElementDrawer.clipSheetHighlight(frozenRowMatch, plan: farPlan, rowCount: 6, columnCount: 6)
        #expect(frozenRowPieces.count == 1)
        #expect(frozenRowPieces[0].clip.minY == farPlan.frozenOriginY)
        let frozenColMatch = CGRect(x: 42, y: 150, width: 60, height: 30)
        #expect(!frozenColMatch.intersects(farViewport))
        let frozenColPieces = CGElementDrawer.clipSheetHighlight(frozenColMatch, plan: farPlan, rowCount: 6, columnCount: 6)
        #expect(frozenColPieces.count == 1)
        #expect(frozenColPieces[0].clip.minX == farPlan.frozenOriginX)
    }

    @Test("PPTX TEXT 元素在紧缩文本框高度下允许纵向溢出不裁切字形，固定 RECT 仍按框裁剪")
    func textElementDoesNotClipOverflowingLineBox() throws {
        let jsonBase: (String) -> Data = { kind in
            """
            {
              "type": "\(kind)",
              "x": 0,
              "y": 0,
              "width": 200,
              "height": 15.45,
              "padding": 0,
              "textWrap": true,
              "verticalAlignment": "TOP",
              "paragraphs": [{
                "alignment": 0,
                "lineSpacingRule": "AUTO",
                "lineSpacing": 1.55,
                "runs": [{ "text": "难点：选股指标迁移ygp", "size": 12.75, "color": 4278190080 }]
              }]
            }
            """.data(using: .utf8)!
        }

        let textEl = try JSONDecoder().decode(OfficeElement.self, from: jsonBase("TEXT"))
        let rectEl = try JSONDecoder().decode(OfficeElement.self, from: jsonBase("RECT"))

        func hasDarkPixelBelow(_ el: OfficeElement, yStart: Int) -> Bool {
            let w = 220, h = 40
            let colorSpace = CGColorSpace(name: CGColorSpace.sRGB) ?? CGColorSpaceCreateDeviceRGB()
            var bytes = [UInt8](repeating: 255, count: w * h * 4)
            let ctx = CGContext(
                data: &bytes,
                width: w,
                height: h,
                bitsPerComponent: 8,
                bytesPerRow: w * 4,
                space: colorSpace,
                bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue
            )!
            ctx.translateBy(x: 0, y: CGFloat(h))
            ctx.scaleBy(x: 1, y: -1)
            CGElementDrawer.drawElement(in: ctx, element: el, images: [:])
            for y in yStart..<h {
                for x in 0..<w {
                    let idx = (y * w + x) * 4
                    if bytes[idx] < 240 || bytes[idx + 1] < 240 || bytes[idx + 2] < 240 {
                        return true
                    }
                }
            }
            return false
        }

        #expect(hasDarkPixelBelow(textEl, yStart: 16))
        #expect(!hasDarkPixelBelow(rectEl, yStart: 17))
    }

    @Test("OfficeElement 解码 cellValigns 与 cellBorders，缺省时回落空数组")
    func tableCellValignsAndBordersDecoding() throws {
        // 1. 显式携带垂直对齐与四边边框
        let json = """
        {
            "type": "TABLE",
            "width": 200,
            "columnWidths": [100, 100],
            "cellValigns": [[0, 2], [1, 0]],
            "cellBorders": [
                [
                    [{"color": 4279312947, "width": 1.5}, {"color": 0, "width": 0}, {"color": 4282668390, "width": 0.75}, {"color": 0, "width": 0}],
                    [{"color": 0, "width": 0}, {"color": 0, "width": 0}, {"color": 4282668390, "width": 0.75}, {"color": 0, "width": 0}]
                ],
                [
                    [{"color": 0, "width": 0}, {"color": 0, "width": 0}, {"color": 4279312947, "width": 1.5}, {"color": 0, "width": 0}],
                    [{"color": 0, "width": 0}, {"color": 0, "width": 0}, {"color": 4279312947, "width": 1.5}, {"color": 0, "width": 0}]
                ]
            ],
            "rows": [
                [[{"runs": [{"text": "A1"}]}], [{"runs": [{"text": "B1"}]}]],
                [[{"runs": [{"text": "A2"}]}], [{"runs": [{"text": "B2"}]}]]
            ]
        }
        """.data(using: .utf8)!

        let element = try JSONDecoder().decode(OfficeElement.self, from: json)
        #expect(element.cellValigns.count == 2)
        #expect(element.cellValigns[0] == [0, 2])
        #expect(element.cellValigns[1] == [1, 0])
        #expect(element.cellBorders.count == 2)
        guard element.cellBorders.count == 2,
              element.cellBorders[0].count == 2,
              element.cellBorders[0][0].count == 4,
              element.cellBorders[1].count == 2 else {
            Issue.record("cellBorders 维度应为 2×2，每个单元格 4 条边")
            return
        }
        // 首行左单元格：top=1.5pt #112233，right 隐藏，bottom=0.75pt #445566，left 隐藏
        #expect(element.cellBorders[0][0][0].color == 0xFF112233)
        #expect(abs(element.cellBorders[0][0][0].width - 1.5) < 0.001)
        #expect(element.cellBorders[0][0][1].width == 0)
        #expect(element.cellBorders[0][0][2].color == 0xFF445566)
        #expect(abs(element.cellBorders[0][0][2].width - 0.75) < 0.001)
        #expect(element.cellBorders[0][0][3].width == 0)
        // 末行左单元格：仅保留 bottom 粗线
        #expect(element.cellBorders[1][0][0].width == 0)
        #expect(element.cellBorders[1][0][2].color == 0xFF112233)
        #expect(abs(element.cellBorders[1][0][2].width - 1.5) < 0.001)

        // 2. 普通表格（无这两个字段）保持空数组，兼容既有 JSON
        let plainJSON = """
        {
            "type": "TABLE",
            "width": 200,
            "rows": [[[{"runs": [{"text": "普通"}]}]]]
        }
        """.data(using: .utf8)!
        let plain = try JSONDecoder().decode(OfficeElement.self, from: plainJSON)
        #expect(plain.cellValigns.isEmpty)
        #expect(plain.cellBorders.isEmpty)

        // 3. 成员初始化参数默认值保持既有调用点可用
        let defaulted = OfficeElement(type: .table)
        #expect(defaulted.cellValigns.isEmpty)
        #expect(defaulted.cellBorders.isEmpty)
    }

    @Test("resolveCellVerticalOffset 返回 Top/Center/Bottom 偏移并缺省按居中")
    func resolveCellVerticalOffsetRules() throws {
        let element = OfficeElement(type: .table, cellValigns: [[0, 1, 2]])
        let cellHeight: CGFloat = 100
        let contentHeight: CGFloat = 20

        // 剩余 80：顶部恒为 5，居中取半，底部预留 5 的内边距
        #expect(CGElementDrawer.resolveCellVerticalOffset(
            element: element, row: 0, col: 0, cellHeight: cellHeight, contentHeight: contentHeight
        ) == 5)
        #expect(CGElementDrawer.resolveCellVerticalOffset(
            element: element, row: 0, col: 1, cellHeight: cellHeight, contentHeight: contentHeight
        ) == 40)
        #expect(CGElementDrawer.resolveCellVerticalOffset(
            element: element, row: 0, col: 2, cellHeight: cellHeight, contentHeight: contentHeight
        ) == 75)

        // 缺省（无 cellValigns）与列越界均按居中处理，兼容既有表格
        let legacy = OfficeElement(type: .table)
        #expect(CGElementDrawer.resolveCellVerticalOffset(
            element: legacy, row: 0, col: 0, cellHeight: cellHeight, contentHeight: contentHeight
        ) == 40)
        #expect(CGElementDrawer.resolveCellVerticalOffset(
            element: element, row: 0, col: 9, cellHeight: cellHeight, contentHeight: contentHeight
        ) == 40)
        #expect(CGElementDrawer.resolveCellVerticalOffset(
            element: element, row: 9, col: 0, cellHeight: cellHeight, contentHeight: contentHeight
        ) == 40)

        // 剩余空间不超过 10 时任何对齐方式都退回 5，避免文字贴边
        for col in 0..<3 {
            #expect(CGElementDrawer.resolveCellVerticalOffset(
                element: element, row: 0, col: col, cellHeight: 30, contentHeight: 20
            ) == 5)
        }
    }

    @Test("三线表按 cellBorders 四边绘制且隐藏竖线，缺省时回退统一网格框")
    func tableCellBorderEdgeDrawing() throws {
        // 三线表：首行保留粗顶线与表头底线，其余边全部隐藏
        func tableJSON(_ borders: String, stroke: String = "0", strokeWidth: String = "0") -> Data {
            """
            {
                "type": "TABLE",
                "width": 200,
                "stroke": \(stroke),
                "strokeWidth": \(strokeWidth),
                "columnWidths": [100, 100],
                "cellBorders": \(borders),
                "rows": [
                    [[{"runs": [{"text": "A"}]}], [{"runs": [{"text": "B"}]}]],
                    [[{"runs": [{"text": "C"}]}], [{"runs": [{"text": "D"}]}]]
                ]
            }
            """.data(using: .utf8)!
        }
        let none = #"{"color": 0, "width": 0}"#
        let headerBottom = #"{"color": 4282668390, "width": 0.75}"#
        let thickTop = #"{"color": 4279312947, "width": 1.5}"#
        let borders = """
        [
            [[\(thickTop), \(none), \(headerBottom), \(none)], [\(thickTop), \(none), \(headerBottom), \(none)]],
            [[\(none), \(none), \(thickTop), \(none)], [\(none), \(none), \(thickTop), \(none)]]
        ]
        """

        let triElement = try JSONDecoder().decode(OfficeElement.self, from: tableJSON(borders))

        let w = 220
        let h = 120
        func render(_ element: OfficeElement) -> [UInt8] {
            let colorSpace = CGColorSpace(name: CGColorSpace.sRGB) ?? CGColorSpaceCreateDeviceRGB()
            var bytes = [UInt8](repeating: 255, count: w * h * 4)
            let ctx = CGContext(
                data: &bytes,
                width: w,
                height: h,
                bitsPerComponent: 8,
                bytesPerRow: w * 4,
                space: colorSpace,
                bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue
            )!
            ctx.translateBy(x: 0, y: CGFloat(h))
            ctx.scaleBy(x: 1, y: -1)
            CGElementDrawer.drawElement(in: ctx, element: element, images: [:])
            return bytes
        }
        func isDark(_ bytes: [UInt8], _ x: Int, _ y: Int) -> Bool {
            guard x >= 0, x < w, y >= 0, y < h else { return false }
            let idx = (y * w + x) * 4
            return bytes[idx] < 240 || bytes[idx + 1] < 240 || bytes[idx + 2] < 240
        }
        /// 统计指定矩形区域内被描边覆盖的像素数，用于判断某条边是否存在
        func darkCount(_ bytes: [UInt8], xRange: Range<Int>, yRange: Range<Int>) -> Int {
            var count = 0
            for y in yRange {
                for x in xRange where isDark(bytes, x, y) {
                    count += 1
                }
            }
            return count
        }

        let triBytes = render(triElement)
        let geom = CGElementDrawer.computeTableGeometry(element: triElement)
        #expect(geom.cols == 2)
        #expect(geom.colX == [0, 100, 200])
        let headerBottomY = Int(geom.rowTops[1].rounded())
        let tableBottomY = Int(geom.rowTops[2].rounded())

        // 顶部粗线、表头底线与表格底线均存在（描边路径居中于边界，取边界上下各 2 像素）
        #expect(darkCount(triBytes, xRange: 20..<180, yRange: 0..<3) > 20)
        #expect(darkCount(triBytes, xRange: 20..<180, yRange: (headerBottomY - 2)..<(headerBottomY + 2)) > 10)
        #expect(darkCount(triBytes, xRange: 20..<180, yRange: (tableBottomY - 3)..<(tableBottomY + 2)) > 20)

        // 两条竖线全部隐藏：列边界处的纵向条带没有任何描边
        let row0Interior = 4..<(headerBottomY - 3)
        let row1Interior = (headerBottomY + 3)..<(tableBottomY - 3)
        #expect(darkCount(triBytes, xRange: 99..<102, yRange: row0Interior) == 0)
        #expect(darkCount(triBytes, xRange: 99..<102, yRange: row1Interior) == 0)
        #expect(darkCount(triBytes, xRange: 0..<3, yRange: row0Interior) == 0)
        #expect(darkCount(triBytes, xRange: 197..<200, yRange: row0Interior) == 0)

        // 未提供 cellBorders 时沿用统一网格框，内部竖线恢复可见
        let plainElement = try JSONDecoder().decode(
            OfficeElement.self,
            from: tableJSON(#"[]"#, stroke: "4278190080", strokeWidth: "1")
        )
        #expect(plainElement.cellBorders.isEmpty)
        let plainBytes = render(plainElement)
        #expect(darkCount(plainBytes, xRange: 99..<103, yRange: row0Interior) > 10)
        #expect(darkCount(plainBytes, xRange: 197..<202, yRange: row0Interior) > 10)
    }

    @Test("Bottom 对齐的单元格在检索高亮与文本选区中与绘制共用同一垂直偏移")
    func tableBottomValignHighlightAndSelectionAlignment() throws {
        // 同一行内用较高单元格抬升行高，使较低的单元格获得足够剩余空间触发居中/底部差异
        func docJSON(_ valigns: String) -> Data {
            """
            {
              "schemaVersion": 9,
              "kind": "DOCX",
              "width": 595,
              "blocks": [{
                "type": "TABLE",
                "width": 300,
                "columnWidths": [150, 150],
                "cellValigns": \(valigns),
                "rows": [[
                  [
                    {"image": "word/media/tall.png", "imageWidth": 100, "imageHeight": 60},
                    {"runs": [{"text": "高格文字", "size": 12, "color": 4278190080}]}
                  ],
                  [{"runs": [{"text": "低格文字", "size": 12, "color": 4278190080}]}]
                ]]
              }]
            }
            """.data(using: .utf8)!
        }

        let centered = try JSONDecoder().decode(OfficeDocument.self, from: docJSON("[[1, 1]]"))
        let bottomed = try JSONDecoder().decode(OfficeDocument.self, from: docJSON("[[1, 2]]"))
        let centeredPlan = CGElementDrawer.layoutDocxBlocks(in: centered)
        let bottomedPlan = CGElementDrawer.layoutDocxBlocks(in: bottomed)
        #expect(centeredPlan.placements.count == 1)
        #expect(bottomedPlan.placements.count == 1)

        // 以实际几何与排版高度复算期望偏移，验证两个消费方确实调用统一方法
        var tbl = centered.blocks[0]
        tbl.width = centeredPlan.placements[0].frame.width
        let geom = CGElementDrawer.computeTableGeometry(element: tbl)
        #expect(geom.cols == 2)
        let cellH = geom.rowTops[1] - geom.rowTops[0]
        let availCellW = max(1, geom.colWidths[1] - 10)
        let shortParas = tbl.rows[0][1]
        let shortContentH = CGElementDrawer.measureParagraphsHeight(
            shortParas,
            availableWidth: availCellW,
            wrap: true
        )
        // 行高被左侧高单元格抬升，剩余空间必须超过 10 才能区分居中和底部
        #expect(cellH - shortContentH > 10)

        let centerOffset = CGElementDrawer.resolveCellVerticalOffset(
            element: tbl, row: 0, col: 1, cellHeight: cellH, contentHeight: shortContentH
        )
        var bottomTbl = bottomed.blocks[0]
        bottomTbl.width = bottomedPlan.placements[0].frame.width
        let bottomOffset = CGElementDrawer.resolveCellVerticalOffset(
            element: bottomTbl, row: 0, col: 1, cellHeight: cellH, contentHeight: shortContentH
        )
        #expect(centerOffset != 5)
        #expect(bottomOffset > centerOffset)
        let delta = bottomOffset - centerOffset

        // 1. 文本选区：底部对齐单元格的块原点整体下移 delta
        let centerLayout = DocumentTextLayout.build(from: centered)
        let bottomLayout = DocumentTextLayout.build(from: bottomed)
        guard let centerBlock = centerLayout.blocks.first(where: { $0.fullText.contains("低格文字") }),
              let bottomBlock = bottomLayout.blocks.first(where: { $0.fullText.contains("低格文字") }) else {
            Issue.record("未在表格中构建出低格文字的文本块")
            return
        }
        let centerY = centerBlock.transform.ty - centeredPlan.placements[0].frame.minY
        let bottomY = bottomBlock.transform.ty - bottomedPlan.placements[0].frame.minY
        #expect(abs(centerY - centerOffset) < 0.5)
        #expect(abs(bottomY - bottomOffset) < 0.5)
        #expect(abs((bottomY - centerY) - delta) < 0.5)

        // 2. 检索高亮：同一字符区间的高亮矩形同步下移 delta
        let match = OfficeSearchMatch(
            pageIndex: 0,
            elementIndex: 0,
            paragraphIndex: 0,
            row: 0,
            column: 1,
            startChar: 0,
            endChar: 4,
            snippet: "低格文字"
        )
        let centerRects = CGElementDrawer.computeHighlightRects(for: match, in: centered, docxPlan: centeredPlan)
        let bottomRects = CGElementDrawer.computeHighlightRects(for: match, in: bottomed, docxPlan: bottomedPlan)
        guard let centerRect = centerRects.first, let bottomRect = bottomRects.first else {
            Issue.record("表格单元格检索命中未产生高亮矩形")
            return
        }
        #expect(abs((bottomRect.minY - centerRect.minY) - delta) < 0.5)
        // 高亮矩形与选区块原点使用同一基准，正文绘制的起排点也因此保持一致
        #expect(abs((centerRect.minY - centeredPlan.placements[0].frame.minY) - centerY) < 0.5)
        #expect(abs((bottomRect.minY - bottomedPlan.placements[0].frame.minY) - bottomY) < 0.5)
    }

    @Test("OfficeElement 解码 rowHeights 与 cellPaddings，缺省为空并清洗非法值")
    func tableRowHeightsAndCellPaddingsDecoding() throws {
        // 1. 显式携带行高下限与四边内边距矩阵
        let json = """
        {
            "type": "TABLE",
            "width": 200,
            "columnWidths": [100, 100],
            "rowHeights": [48, 24],
            "cellPaddings": [
                [[0, 12, 8, 16], [3.6, 7.2, 3.6, 7.2]],
                [[5, 5, 5, 5], [12, 0, 4, 2]]
            ],
            "rows": [
                [[{"runs": [{"text": "A1"}]}], [{"runs": [{"text": "B1"}]}]],
                [[{"runs": [{"text": "A2"}]}], [{"runs": [{"text": "B2"}]}]]
            ]
        }
        """.data(using: .utf8)!

        let element = try JSONDecoder().decode(OfficeElement.self, from: json)
        #expect(element.rowHeights == [48, 24])
        #expect(element.cellPaddings.count == 2)
        #expect(element.cellPaddings[0].count == 2)
        #expect(element.cellPaddings[0][0] == [0, 12, 8, 16])
        #expect(element.cellPaddings[0][1] == [3.6, 7.2, 3.6, 7.2])
        #expect(element.cellPaddings[1][1] == [12, 0, 4, 2])

        // 2. 缺省时保持空数组，兼容既有 schemaVersion = 9 JSON
        let plainJSON = """
        {
            "type": "TABLE",
            "width": 200,
            "rows": [[[{"runs": [{"text": "普通"}]}]]]
        }
        """.data(using: .utf8)!
        let plain = try JSONDecoder().decode(OfficeElement.self, from: plainJSON)
        #expect(plain.rowHeights.isEmpty)
        #expect(plain.cellPaddings.isEmpty)

        // 3. 非法值清洗：负数钳位到 0、超界钳位到上限、
        //    非 4 元边距所在行整体回退为空（保留行索引对齐，该行按缺省 [5, 5, 5, 5] 处理）
        let dirtyJSON = """
        {
            "type": "TABLE",
            "width": 200,
            "rowHeights": [-5, 20000, 30],
            "cellPaddings": [
                [[-1, 12, 8, 16], [3.6, 7.2, 3.6, 7.2]],
                [[1, 2, 3], [5, 5, 5, 5000]]
            ],
            "rows": [
                [[{"runs": [{"text": "A1"}]}], [{"runs": [{"text": "B1"}]}]],
                [[{"runs": [{"text": "A2"}]}], [{"runs": [{"text": "B2"}]}]]
            ]
        }
        """.data(using: .utf8)!
        let dirty = try JSONDecoder().decode(OfficeElement.self, from: dirtyJSON)
        // 负值钳位到 0，超界钳位到 14400
        #expect(dirty.rowHeights == [0, 14400, 30])
        // 第 2 行含长度不为 4 的边距，整行回退为空；第 1 行保留并把 -1 钳位到 0
        #expect(dirty.cellPaddings.count == 2)
        #expect(dirty.cellPaddings[0][0] == [0, 12, 8, 16])
        #expect(dirty.cellPaddings[0][1] == [3.6, 7.2, 3.6, 7.2])
        #expect(dirty.cellPaddings[1].isEmpty)
        // 回退的行在查询侧落到缺省内边距
        let dirtyPad = CGElementDrawer.cellPadding(element: dirty, row: 1, col: 0)
        #expect(dirtyPad.top == 5 && dirtyPad.right == 5 && dirtyPad.bottom == 5 && dirtyPad.left == 5)

        // 4. 成员初始化参数默认值保持既有调用点可用
        let defaulted = OfficeElement(type: .table)
        #expect(defaulted.rowHeights.isEmpty)
        #expect(defaulted.cellPaddings.isEmpty)
    }

    @Test("cellPadding 查询缺省 [5,5,5,5]，computeTableGeometry 按 rowHeights 与纵向合并差额分配行高")
    func tableRowHeightsAndCellPaddingGeometry() throws {
        // 1. cellPadding：缺省、界内、越界与非法行各分支
        let plain = OfficeElement(type: .table)
        let plainPad = CGElementDrawer.cellPadding(element: plain, row: 0, col: 0)
        #expect(plainPad.top == 5 && plainPad.right == 5 && plainPad.bottom == 5 && plainPad.left == 5)

        let padded = OfficeElement(
            type: .table,
            width: 200,
            columnWidths: [100, 100],
            cellPaddings: [[[0, 12, 8, 16], [5, 5, 5, 5]]]
        )
        let pad00 = CGElementDrawer.cellPadding(element: padded, row: 0, col: 0)
        #expect(pad00.top == 0 && pad00.right == 12 && pad00.bottom == 8 && pad00.left == 16)
        // 越界行列回退缺省
        let outOfRange = CGElementDrawer.cellPadding(element: padded, row: 9, col: 9)
        #expect(outOfRange.top == 5 && outOfRange.left == 5)

        // 2. rowHeights = [48, 24] 在短文本下维持至少 48 / 24 的行高
        let shortText = #"[{"runs": [{"text": "短"}]}]"#
        let rowHeightsJSON = """
        {
            "type": "TABLE",
            "width": 200,
            "columnWidths": [100, 100],
            "rowHeights": [48, 24],
            "rows": [[\(shortText)], [\(shortText)]]
        }
        """.data(using: .utf8)!
        let rowHeightsElement = try JSONDecoder().decode(OfficeElement.self, from: rowHeightsJSON)
        let geom = CGElementDrawer.computeTableGeometry(element: rowHeightsElement)
        #expect(geom.rowTops.count == 3)
        #expect(abs((geom.rowTops[1] - geom.rowTops[0]) - 48) < 0.5)
        #expect(abs((geom.rowTops[2] - geom.rowTops[1]) - 24) < 0.5)

        // 3. 非对称内边距参与行高：padTop + padBottom 计入内容高度
        let tallPadJSON = """
        {
            "type": "TABLE",
            "width": 200,
            "columnWidths": [200],
            "cellPaddings": [[[30, 0, 20, 0]]],
            "rows": [[\(shortText)]]
        }
        """.data(using: .utf8)!
        let tallPadElement = try JSONDecoder().decode(OfficeElement.self, from: tallPadJSON)
        let tallGeom = CGElementDrawer.computeTableGeometry(element: tallPadElement)
        let textOnlyH = CGElementDrawer.measureParagraphsHeight(
            tallPadElement.rows[0][0], availableWidth: 200, wrap: true
        )
        // 行高至少为 文本高度 + padTop + padBottom（30 + 20）
        #expect(tallGeom.rowTops[1] >= textOnlyH + 50 - 0.5)

        // 4. 纵向合并：两行显式 rowHeights 总高已足够时不虚增首行，差额补齐到末行
        let mergeJSON = """
        {
            "type": "TABLE",
            "width": 200,
            "columnWidths": [100, 100],
            "rowHeights": [40, 40],
            "merges": [{"startRow": 0, "startColumn": 0, "endRow": 1, "endColumn": 0}],
            "rows": [
                [[{"runs": [{"text": "纵向合并内容"}]}], [{"runs": [{"text": "B1"}]}]],
                [[], [{"runs": [{"text": "B2"}]}]]
            ]
        }
        """.data(using: .utf8)!
        let mergeElement = try JSONDecoder().decode(OfficeElement.self, from: mergeJSON)
        let mergeGeom = CGElementDrawer.computeTableGeometry(element: mergeElement)
        let spanH = mergeGeom.rowTops[2] - mergeGeom.rowTops[0]
        let mergedTextH = CGElementDrawer.measureParagraphsHeight(
            mergeElement.rows[0][0], availableWidth: 100 - 10, wrap: true
        )
        // 两行显式高度合计 80 已足够容纳合并内容，首行保持 40 不被压缩/拉高
        #expect(mergedTextH + 10 <= 80)
        #expect(abs((mergeGeom.rowTops[1] - mergeGeom.rowTops[0]) - 40) < 0.5)
        #expect(abs(spanH - 80) < 0.5)

        // 5. 纵向合并：显式高度不足时差额只补到末行，首行维持原值
        let shortMergeJSON = """
        {
            "type": "TABLE",
            "width": 200,
            "columnWidths": [100, 100],
            "rowHeights": [20, 20],
            "merges": [{"startRow": 0, "startColumn": 0, "endRow": 1, "endColumn": 0}],
            "rows": [
                [[{"runs": [{"text": "纵向合并内容较长需要更多纵向空间", "size": 24}]}], [{"runs": [{"text": "B1"}]}]],
                [[], [{"runs": [{"text": "B2"}]}]]
            ]
        }
        """.data(using: .utf8)!
        let shortMergeElement = try JSONDecoder().decode(OfficeElement.self, from: shortMergeJSON)
        let shortGeom = CGElementDrawer.computeTableGeometry(element: shortMergeElement)
        let shortMergedTextH = CGElementDrawer.measureParagraphsHeight(
            shortMergeElement.rows[0][0], availableWidth: 100 - 10, wrap: true
        )
        // 首行只由非纵向跨越高度的 (0,1) 决定：显式行高 20 与「内容高 + 上下内边距 10」取较大值；
        // 纵向合并的 (0,0) 不参与首行撑高，其差额全部由末行吸收
        let sideCellH = CGElementDrawer.measureParagraphsHeight(
            shortMergeElement.rows[0][1], availableWidth: 100 - 10, wrap: true
        )
        let expectedFirstRow = max(20, sideCellH + 10)
        #expect(abs((shortGeom.rowTops[1] - shortGeom.rowTops[0]) - expectedFirstRow) < 0.5)
        #expect(abs((shortGeom.rowTops[2] - shortGeom.rowTops[0]) - (shortMergedTextH + 10)) < 0.5)
    }

    @Test("自定义 cellPaddings 下 PPTX/DOCX 检索高亮与文本选区坐标与绘制严格对齐")
    func tableCellPaddingHighlightAndSelectionAlignment() throws {
        // 非对称内边距：左 16 / 右 12 / 上 8 / 下 8，行高下限 48 拉出充足剩余空间
        let padding = #"[[[0, 12, 8, 16]]]"#
        func pptxJSON(valign: Int, paddings: String) -> Data {
            """
            {
              "schemaVersion": 9,
              "kind": "PPTX",
              "width": 720,
              "pages": [{
                "width": 720,
                "height": 405,
                "elements": [{
                  "type": "TABLE",
                  "width": 200,
                  "columnWidths": [200],
                  "rowHeights": [48],
                  "cellValigns": [[\(valign)]],
                  "cellPaddings": \(paddings),
                  "rows": [[[{"runs": [{"text": "对齐校验", "size": 12, "color": 4278190080}]}]]]
                }]
              }]
            }
            """.data(using: .utf8)!
        }
        func docxJSON(valign: Int, paddings: String) -> Data {
            """
            {
              "schemaVersion": 9,
              "kind": "DOCX",
              "width": 595,
              "blocks": [{
                "type": "TABLE",
                "width": 200,
                "columnWidths": [200],
                "rowHeights": [48],
                "cellValigns": [[\(valign)]],
                "cellPaddings": \(paddings),
                "rows": [[[{"runs": [{"text": "对齐校验", "size": 12, "color": 4278190080}]}]]]
              }]
            }
            """.data(using: .utf8)!
        }

        // 逐分支验证：Top(0) / Center(1) / Bottom(2) 三种垂直对齐都要对齐
        for valign in 0...2 {
            let pptxDoc = try JSONDecoder().decode(OfficeDocument.self, from: pptxJSON(valign: valign, paddings: padding))
            let docxDoc = try JSONDecoder().decode(OfficeDocument.self, from: docxJSON(valign: valign, paddings: padding))
            let element = pptxDoc.pages[0].elements[0]
            let geom = CGElementDrawer.computeTableGeometry(element: element)
            let cellH = geom.rowTops[1] - geom.rowTops[0]
            let cellW = geom.colX[1] - geom.colX[0]
            let pad = CGElementDrawer.cellPadding(element: element, row: 0, col: 0)
            #expect(pad.top == 0 && pad.right == 12 && pad.bottom == 8 && pad.left == 16)
            let availW = max(1, cellW - pad.left - pad.right)
            let contentH = CGElementDrawer.measureParagraphsHeight(
                element.rows[0][0], availableWidth: availW, wrap: true
            )
            let vOffset = CGElementDrawer.resolveCellVerticalOffset(
                element: element, row: 0, col: 0, cellHeight: cellH, contentHeight: contentH
            )

            let match = OfficeSearchMatch(
                pageIndex: 0, elementIndex: 0, paragraphIndex: 0, row: 0, column: 0,
                startChar: 0, endChar: 4, snippet: "对齐校验"
            )

            // 1. PPTX 检索高亮：x 起点 = 单元格左边界 + padLeft，y 起点 = 行顶 + vOffset
            let pptxRects = CGElementDrawer.computeHighlightRects(for: match, in: pptxDoc)
            guard let pptxRect = pptxRects.first else {
                Issue.record("PPTX 表格检索命中未产生高亮矩形（valign=\(valign)）")
                return
            }
            #expect(abs(pptxRect.minX - pad.left) < 0.5,
                    "PPTX 高亮左边界应按 padLeft=16 起排（valign=\(valign)），实际 \(pptxRect.minX)")
            #expect(abs(pptxRect.minY - vOffset) < 0.5,
                    "PPTX 高亮顶部应按 vOffset 对齐（valign=\(valign)）")

            // 2. PPTX 文本选区：选区块 transform 原点必须与高亮同源
            let pptxLayout = DocumentTextLayout.build(from: pptxDoc)
            guard let pptxBlock = pptxLayout.blocks.first(where: { $0.fullText.contains("对齐校验") }) else {
                Issue.record("PPTX 表格未构建出文本块（valign=\(valign)）")
                return
            }
            #expect(abs(pptxBlock.transform.tx - pad.left) < 0.5,
                    "PPTX 选区原点 x 应与绘制共用 padLeft（valign=\(valign)）")
            #expect(abs(pptxBlock.transform.ty - vOffset) < 0.5,
                    "PPTX 选区原点 y 应与绘制共用 vOffset（valign=\(valign)）")

            // 3. DOCX 检索高亮与文本选区：同一套内边距基准
            let docxPlan = CGElementDrawer.layoutDocxBlocks(in: docxDoc)
            guard let placement = docxPlan.placements.first else {
                Issue.record("DOCX 表格未产生 placement（valign=\(valign)）")
                return
            }
            let docxRects = CGElementDrawer.computeHighlightRects(for: match, in: docxDoc, docxPlan: docxPlan)
            guard let docxRect = docxRects.first else {
                Issue.record("DOCX 表格检索命中未产生高亮矩形（valign=\(valign)）")
                return
            }
            #expect(abs((docxRect.minX - placement.frame.minX) - pad.left) < 0.5,
                    "DOCX 高亮左边界应按 padLeft 起排（valign=\(valign)）")
            #expect(abs((docxRect.minY - placement.frame.minY) - vOffset) < 0.5,
                    "DOCX 高亮顶部应按 vOffset 对齐（valign=\(valign)）")

            let docxLayout = DocumentTextLayout.build(from: docxDoc)
            guard let docxBlock = docxLayout.blocks.first(where: { $0.fullText.contains("对齐校验") }) else {
                Issue.record("DOCX 表格未构建出文本块（valign=\(valign)）")
                return
            }
            #expect(abs((docxBlock.transform.tx - placement.frame.minX) - pad.left) < 0.5,
                    "DOCX 选区原点 x 应与绘制共用 padLeft（valign=\(valign)）")
            #expect(abs((docxBlock.transform.ty - placement.frame.minY) - vOffset) < 0.5,
                    "DOCX 选区原点 y 应与绘制共用 vOffset（valign=\(valign)）")
        }

        // 4. 端到端：离屏绘制时文字实际像素同样按 padLeft 起排，不回落到 5pt 老位置
        let w = 260
        let h = 80
        func render(_ el: OfficeElement) -> [UInt8] {
            let colorSpace = CGColorSpace(name: CGColorSpace.sRGB) ?? CGColorSpaceCreateDeviceRGB()
            var bytes = [UInt8](repeating: 255, count: w * h * 4)
            let ctx = CGContext(
                data: &bytes, width: w, height: h, bitsPerComponent: 8, bytesPerRow: w * 4,
                space: colorSpace, bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue
            )!
            ctx.translateBy(x: 0, y: CGFloat(h))
            ctx.scaleBy(x: 1, y: -1)
            CGElementDrawer.drawElement(in: ctx, element: el, images: [:])
            return bytes
        }
        func firstDarkColumn(_ bytes: [UInt8]) -> Int? {
            // 跳过表格外框本身（左边界线宽约 1pt，且描边在 0..1 像素内），
            // 否则两种内边距都会在 x=0 命中边框墨迹而失去区分度
            for x in 2..<w {
                for y in 0..<h {
                    let idx = (y * w + x) * 4
                    if bytes[idx] < 200 || bytes[idx + 1] < 200 || bytes[idx + 2] < 200 {
                        return x
                    }
                }
            }
            return nil
        }
        // 从 PPTX 文档里取表格元素做离屏绘制，避免单独构造时漏掉同样的字段
        let paddedTable = try JSONDecoder().decode(OfficeDocument.self, from: pptxJSON(valign: 0, paddings: padding))
            .pages[0].elements[0]
        let paddedFirstDark = firstDarkColumn(render(paddedTable))

        let legacyTable = try JSONDecoder().decode(OfficeDocument.self, from: pptxJSON(valign: 0, paddings: "[]"))
            .pages[0].elements[0]
        let legacyFirstDark = firstDarkColumn(render(legacyTable))

        guard let paddedX = paddedFirstDark, let legacyX = legacyFirstDark else {
            Issue.record("离屏绘制未产生任何文字像素")
            return
        }
        // 缺省内边距左 5pt，自定义左 16pt：首列墨迹应整体右移约 11pt
        #expect(abs(CGFloat(paddedX - legacyX) - 11) < 3,
                "自定义 padLeft=16 的绘制起排应比缺省 padLeft=5 右移约 11pt，实际 \(paddedX) vs \(legacyX)")
    }

    @Test("strokeDash 虚线描边解码与离屏绘制")
    func strokeDashDecodingAndRendering() throws {
        // 1. 解码：含 strokeDash 的元素
        let dashedJSON = """
        {"type":"LINE","x":0,"y":0,"width":100,"height":0,"transform":[1,0,0,1,0,0],
        "padding":0,"fill":0,"stroke":4278190080,"strokeWidth":2,
        "verticalAlignment":"TOP","textWrap":true,"strokeDash":[8.0,6.0]}
        """.data(using: .utf8)!
        let dashed = try JSONDecoder().decode(OfficeElement.self, from: dashedJSON)
        #expect(dashed.strokeDash == [8, 6])

        // 2. 解码：缺少该字段时回落为空数组（实线），保证旧文档 JSON 结构兼容
        let plainJSON = """
        {"type":"LINE","x":0,"y":0,"width":100,"height":0,"transform":[1,0,0,1,0,0],
        "padding":0,"fill":0,"stroke":4278190080,"strokeWidth":2,
        "verticalAlignment":"TOP","textWrap":true}
        """.data(using: .utf8)!
        let plain = try JSONDecoder().decode(OfficeElement.self, from: plainJSON)
        #expect(plain.strokeDash.isEmpty)

        // 3. 解码：非法序列（长度不足 / 非正数）整体回退为实线
        for invalid in ["[8.0]", "[0,6.0]", "[-1,6.0]"] {
            let json = """
            {"type":"LINE","x":0,"y":0,"width":100,"height":0,"transform":[1,0,0,1,0,0],
            "padding":0,"fill":0,"stroke":4278190080,"strokeWidth":2,
            "verticalAlignment":"TOP","textWrap":true,"strokeDash":\(invalid)}
            """.data(using: .utf8)!
            let element = try JSONDecoder().decode(OfficeElement.self, from: json)
            #expect(element.strokeDash.isEmpty, "非法序列 \(invalid) 应回退为实线")
        }

        // 4. 离屏绘制：虚线在描边路径上产生间断，实线则连续
        func strokeRunLengths(dash: [CGFloat]) -> [Int] {
            let colorSpace = CGColorSpace(name: CGColorSpace.sRGB) ?? CGColorSpaceCreateDeviceRGB()
            let width = 240, height = 40
            var bytes = [UInt8](repeating: 255, count: width * height * 4)
            let ctx = CGContext(
                data: &bytes,
                width: width,
                height: height,
                bitsPerComponent: 8,
                bytesPerRow: width * 4,
                space: colorSpace,
                bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue
            )!
            let element = OfficeElement(
                type: .line,
                x: 0,
                y: 0,
                width: 200,
                height: 0,
                padding: 0,
                stroke: 0xff000000,
                strokeWidth: 3,
                strokeDash: dash
            )
            CGElementDrawer.drawElement(in: ctx, element: element, images: [:])
            // drawElement 会应用 transform 矩阵，线段沿 (0,0)->(200,0) 绘制；
            // 逐行扫描并取描边像素最多的那一行，统计连续被覆盖的像素段长度（即虚线段数）。
            var best: [Int] = []
            for y in 0..<height {
                var runs: [Int] = []
                var run = 0
                for x in 0..<width {
                    let inked = bytes[(y * width + x) * 4] < 200
                    if inked {
                        run += 1
                    } else if run > 0 {
                        runs.append(run)
                        run = 0
                    }
                }
                if run > 0 { runs.append(run) }
                if runs.reduce(0, +) > best.reduce(0, +) { best = runs }
            }
            return best
        }

        let solidRuns = strokeRunLengths(dash: [])
        #expect(solidRuns.count <= 2, "实线应为一到两段连续描边，实际 \(solidRuns)")

        let dashRuns = strokeRunLengths(dash: [8, 6])
        #expect(dashRuns.count >= 4, "虚线应产生多段间断描边，实际 \(dashRuns)")
        #expect(dashRuns.allSatisfy { $0 > 0 })

        // 5. 非法点线序列按实线绘制且不崩溃
        let fallbackRuns = strokeRunLengths(dash: [0, -1])
        #expect(fallbackRuns.count <= 2, "非法序列应按实线绘制，实际 \(fallbackRuns)")
    }

    // MARK: - XLSX 单元格选区与 TSV 复制（对齐 scripts/tests/SheetSelectionChecks.java）

    /// 与 Java 参考套件一致的标头常量（Android 取 42/20，macOS 绘制常量为 42/22）
    private enum SheetFixture {
        static let headerWidth: CGFloat = 42
        static let headerHeight: CGFloat = 20
        static let rowHeight: CGFloat = 20
        static let columnWidth: CGFloat = 64
    }

    /// 构造工作表：所有行高、列宽相同，按行列二维数组填充文本（`nil` 表示该格无单元格）
    private func makeSheet(
        grid: [[String?]],
        rowHeight: CGFloat = SheetFixture.rowHeight,
        columnWidth: CGFloat = SheetFixture.columnWidth,
        merges: [OfficeCellRange] = []
    ) -> OfficeSheet {
        let rowCount = grid.count
        let columnCount = rowCount == 0 ? 0 : grid[0].count
        var cells: [OfficeCell] = []
        // 单元格必须按行优先、行内列递增追加，满足 rowStarts 契约
        for row in 0..<rowCount {
            for column in 0..<columnCount {
                guard let text = grid[row][column] else { continue }
                cells.append(OfficeCell(
                    row: row,
                    column: column,
                    text: text,
                    numeric: !text.isEmpty && Double(text) != nil
                ))
            }
        }
        return OfficeSheet(
            name: "Fixture",
            rowHeights: [CGFloat](repeating: rowHeight, count: rowCount),
            columnWidths: [CGFloat](repeating: columnWidth, count: columnCount),
            cells: cells,
            merges: merges
        )
    }

    private func makeLayout(_ sheet: OfficeSheet) -> SheetSelectionModel.SheetSelectionLayout {
        SheetSelectionModel.SheetSelectionLayout(
            sheet: sheet,
            headerWidth: SheetFixture.headerWidth,
            headerHeight: SheetFixture.headerHeight
        )
    }

    private func at(_ row: Int, _ column: Int) -> SheetSelectionModel.CellCoord {
        SheetSelectionModel.CellCoord(row: row, column: column)
    }

    @Test("XLSX 合并单元格不动点闭包扩张（单区域与串联区域）")
    func sheetSelectionMergeClosure() {
        // 1. 单格落在合并区域内：闭包把选区拉回完整合并区域
        let single = makeSheet(
            grid: [["A1", "B1", "C1"], ["A2", "B2", "C2"], ["A3", "B3", "C3"]],
            merges: [OfficeCellRange(startRow: 0, startColumn: 0, endRow: 1, endColumn: 1)]
        )
        let selection = SheetSelectionModel.resolve(sheet: single, anchor: at(1, 1), focus: at(1, 1))
        #expect(selection != nil)
        #expect(selection?.startRow == 0)
        #expect(selection?.startColumn == 0)
        #expect(selection?.endRow == 1)
        #expect(selection?.endColumn == 1)
        // 完全重合的单合并格视为逻辑单格
        #expect(SheetSelectionModel.isSingleLogicalCell(sheet: single, selection: selection))

        let layout = makeLayout(single)
        let rect = SheetSelectionModel.docRect(layout: layout, selection: selection)
        #expect(rect != nil)
        #expect(Int(rect!.left.rounded()) == Int(SheetFixture.headerWidth.rounded()))
        #expect(Int(rect!.right.rounded()) == Int((SheetFixture.headerWidth + 128).rounded()))
        #expect(Int(rect!.top.rounded()) == Int(SheetFixture.headerHeight.rounded()))
        #expect(Int(rect!.bottom.rounded()) == Int((SheetFixture.headerHeight + 40).rounded()))

        // 2. 两个合并区域串联：单轮遍历不足，必须迭代到不动点
        let chained = makeSheet(
            grid: [
                ["A1", "B1", "C1", "D1"],
                ["A2", "B2", "C2", "D2"],
                ["A3", "B3", "C3", "D3"],
            ],
            merges: [
                OfficeCellRange(startRow: 0, startColumn: 0, endRow: 0, endColumn: 2), // A1:C1
                OfficeCellRange(startRow: 0, startColumn: 2, endRow: 0, endColumn: 3), // C1:D1
                OfficeCellRange(startRow: 1, startColumn: 0, endRow: 1, endColumn: 1), // A2:B2
            ]
        )
        // 起点只在 A1：第一轮并入 A1:C1，第二轮因 C1 相交并入 C1:D1，稳定在 A1:D1
        let chain = SheetSelectionModel.resolve(sheet: chained, anchor: at(0, 0), focus: at(0, 0))
        #expect(chain?.startRow == 0)
        #expect(chain?.startColumn == 0)
        #expect(chain?.endRow == 0)
        #expect(chain?.endColumn == 3)
        #expect(!SheetSelectionModel.isSingleLogicalCell(sheet: chained, selection: chain))

        // 触到底部合并区域：闭包下扩到 A2:B2 的末列
        let lower = SheetSelectionModel.resolve(sheet: chained, anchor: at(1, 0), focus: at(1, 0))
        #expect(lower?.startRow == 1)
        #expect(lower?.endRow == 1)
        #expect(lower?.startColumn == 0)
        #expect(lower?.endColumn == 1)

        // 未与任何合并区域相交的普通矩形保持原样
        let plain = SheetSelectionModel.resolve(sheet: chained, anchor: at(2, 3), focus: at(2, 3))
        #expect(plain?.startRow == 2)
        #expect(plain?.endRow == 2)
        #expect(plain?.startColumn == 3)
        #expect(plain?.endColumn == 3)

        // 3. 合并区域**声明顺序倒置**：单轮遍历只能扩到中间区域，必须迭代到不动点。
        // 参考套件的区间恰好按从左到右声明，单轮遍历也能收敛，无法区分"单轮"与"不动点"；
        // 这里把最右侧区域排到最前，并把区间设为真正**相交**（共用 C1 / D1），
        // 于是只有循环到稳定才能扩到 E 列，单轮实现会停在 C 列。
        let reversedOrder = makeSheet(
            grid: [["A1", "B1", "C1", "D1", "E1"]],
            merges: [
                // 最右侧区域在前：起点为 A1 时第一轮与它不相交
                OfficeCellRange(startRow: 0, startColumn: 3, endRow: 0, endColumn: 4), // D1:E1
                OfficeCellRange(startRow: 0, startColumn: 2, endRow: 0, endColumn: 3), // C1:D1
                OfficeCellRange(startRow: 0, startColumn: 0, endRow: 0, endColumn: 2), // A1:C1
            ]
        )
        let fixpoint = SheetSelectionModel.resolve(sheet: reversedOrder, anchor: at(0, 0), focus: at(0, 0))
        #expect(fixpoint?.startColumn == 0)
        #expect(fixpoint?.endColumn == 4, "倒序且相交声明的合并区域必须迭代到不动点才能扩到 E 列")
    }

    @Test("XLSX 反向拖选规范化边界并保留锚点焦点")
    func sheetSelectionReversedDrag() {
        let sheet = makeSheet(grid: [
            ["0,0", "0,1", "0,2"],
            ["1,0", "1,1", "1,2"],
            ["2,0", "2,1", "2,2"],
        ])
        let selection = SheetSelectionModel.resolve(sheet: sheet, anchor: at(2, 2), focus: at(0, 0))
        #expect(selection?.startRow == 0)
        #expect(selection?.startColumn == 0)
        #expect(selection?.endRow == 2)
        #expect(selection?.endColumn == 2)
        #expect(selection?.anchor == at(2, 2))
        #expect(selection?.focus == at(0, 0))
        #expect(selection?.rowCount == 3)
        #expect(selection?.columnCount == 3)
        // 锚点或焦点缺失时不做闭包，直接返回 nil
        #expect(SheetSelectionModel.resolve(sheet: sheet, anchor: nil, focus: at(0, 0)) == nil)
        // CellCoord 构造时把负数钳位到 0
        #expect(SheetSelectionModel.CellCoord(row: -3, column: -7) == at(0, 0))
    }

    @Test("XLSX 隐藏行列整条跳过且不留多余制表符")
    func sheetSelectionHiddenRowsAndColumns() {
        var sheet = makeSheet(grid: [
            ["A1", "hidden", "C1"],
            ["hiddenRow", "hiddenRow", "hiddenRow"],
            ["A3", "hidden", "C3"],
        ])
        // B 列隐藏、第 2 行隐藏（需重建成 let 属性 → 用新的尺寸数组替换）
        var heights = sheet.rowHeights
        heights[1] = 0
        var widths = sheet.columnWidths
        widths[1] = 0
        sheet = OfficeSheet(
            name: sheet.name,
            rowHeights: heights,
            columnWidths: widths,
            cells: sheet.cells,
            merges: sheet.merges,
            geometry: sheet.geometry
        )
        let layout = makeLayout(sheet)
        // 隐藏行列不出现在 axis.visible 中
        #expect(layout.rows.visible.count == 2)
        #expect(layout.columns.visible.count == 2)

        let selection = SheetSelectionModel.resolve(sheet: sheet, anchor: at(0, 0), focus: at(2, 2))
        let text = SheetSelectionModel.selectedText(sheet: sheet, layout: layout, selection: selection)
        #expect(text == "A1\tC1\nA3\tC3", "隐藏行列应整条跳过，实际 <\(text)>")

        // 隐藏列上的单元格仍可被直接寻址（模型层不做额外过滤）
        #expect(SheetSelectionModel.cellTextAt(sheet: sheet, layout: layout, row: 0, column: 1) == "hidden")
    }

    @Test("XLSX 单格与单合并格输出裸文本，多格输出矩形网格")
    func sheetSelectionSingleAndMultiCellText() {
        let sheet = makeSheet(
            grid: [["标题", nil, nil], ["x", "y", "z"]],
            merges: [OfficeCellRange(startRow: 0, startColumn: 0, endRow: 0, endColumn: 2)]
        )
        let layout = makeLayout(sheet)

        // 单格：裸文本，不带制表符
        let single = SheetSelectionModel.resolve(sheet: sheet, anchor: at(1, 1), focus: at(1, 1))
        #expect(SheetSelectionModel.selectedText(sheet: sheet, layout: layout, selection: single) == "y")

        // 单合并格：同样裸文本
        let merged = SheetSelectionModel.resolve(sheet: sheet, anchor: at(0, 1), focus: at(0, 1))
        #expect(SheetSelectionModel.isSingleLogicalCell(sheet: sheet, selection: merged))
        #expect(SheetSelectionModel.selectedText(sheet: sheet, layout: layout, selection: merged) == "标题")

        // 多格：合并区域从属位置留空，保持矩形对齐
        let multi = SheetSelectionModel.resolve(sheet: sheet, anchor: at(0, 0), focus: at(1, 2))
        let text = SheetSelectionModel.selectedText(sheet: sheet, layout: layout, selection: multi)
        #expect(text == "标题\t\t\nx\ty\tz", "合并从属位置应留空，实际 <\(text)>")

        // 空选区语义
        #expect(SheetSelectionModel.selectedText(sheet: sheet, layout: layout, selection: nil) == "")
    }

    @Test("XLSX TSV 按 RFC 4180 转义制表符/换行/回车/双引号")
    func sheetSelectionTsvEscaping() {
        #expect(SheetSelectionModel.formatTsvCell(nil) == "")
        #expect(SheetSelectionModel.formatTsvCell("") == "")
        #expect(SheetSelectionModel.formatTsvCell("plain") == "plain")
        #expect(SheetSelectionModel.formatTsvCell("a\tb") == "\"a\tb\"")
        #expect(SheetSelectionModel.formatTsvCell("a\nb") == "\"a\nb\"")
        #expect(SheetSelectionModel.formatTsvCell("a\rb") == "\"a\rb\"")
        #expect(SheetSelectionModel.formatTsvCell("say \"hi\"") == "\"say \"\"hi\"\"\"")
        #expect(SheetSelectionModel.formatTsvCell("\"") == "\"\"\"\"")
        // CRLF 必须按回车触发包裹：Swift 会把 "\r\n" 聚成单个字形簇，
        // 若按 Character 比较会漏判，导致复制的 TSV 被下游解析成两行
        #expect(SheetSelectionModel.formatTsvCell("a\r\nb") == "\"a\r\nb\"", "CRLF 应触发引号包裹")

        let sheet = makeSheet(grid: [["a\tb", "c"], ["d", "e"]])
        let layout = makeLayout(sheet)
        let selection = SheetSelectionModel.resolve(sheet: sheet, anchor: at(0, 0), focus: at(1, 1))
        let text = SheetSelectionModel.selectedText(sheet: sheet, layout: layout, selection: selection)
        #expect(text == "\"a\tb\"\tc\nd\te", "带引号的单元格应保持网格形状，实际 <\(text)>")
    }

    @Test("XLSX 全空选区导出空串")
    func sheetSelectionAllBlankReturnsEmpty() {
        let sheet = makeSheet(grid: [["only", nil, nil], [nil, nil, nil]])
        let layout = makeLayout(sheet)
        // 只能选中右下角空白矩形：A1 含文本但不在范围内
        let blank = SheetSelectionModel.SheetSelection(
            anchor: at(1, 1),
            focus: at(1, 2),
            startRow: 1,
            startColumn: 1,
            endRow: 1,
            endColumn: 2
        )
        #expect(SheetSelectionModel.selectedText(sheet: sheet, layout: layout, selection: blank) == "")

        // 显式空串单元格同样视为空
        let withEmpty = makeSheet(grid: [["", ""]])
        let emptyLayout = makeLayout(withEmpty)
        let emptyRange = SheetSelectionModel.SheetSelection(
            anchor: at(0, 0),
            focus: at(0, 1),
            startRow: 0,
            startColumn: 0,
            endRow: 0,
            endColumn: 1
        )
        #expect(SheetSelectionModel.selectedText(sheet: withEmpty, layout: emptyLayout, selection: emptyRange) == "")
    }

    @Test("XLSX 冻结窗格命中测试：无冻结 / 冻结列 / 冻结行 / 交叉角")
    func sheetSelectionHitTestFreezeQuadrants() {
        // 1. 无冻结：等价于「文档坐标 = 视图坐标 + 滚动量」
        let plain = makeSheet(grid: [
            ["0,0", "0,1", "0,2"],
            ["1,0", "1,1", "1,2"],
            ["2,0", "2,1", "2,2"],
        ])
        let plainLayout = makeLayout(plain)
        var hit = SheetSelectionModel.hitTest(
            layout: plainLayout, effFrozenRows: 0, effFrozenCols: 0,
            frozenDataW: 0, frozenDataH: 0, scrollDataX: 0, scrollDataY: 0,
            viewX: SheetFixture.headerWidth + 1, viewY: SheetFixture.headerHeight + 1,
            clampToBounds: false
        )
        #expect(hit == at(0, 0), "无冻结应命中 A1")
        hit = SheetSelectionModel.hitTest(
            layout: plainLayout, effFrozenRows: 0, effFrozenCols: 0,
            frozenDataW: 0, frozenDataH: 0, scrollDataX: 64, scrollDataY: 20,
            viewX: SheetFixture.headerWidth + 1, viewY: SheetFixture.headerHeight + 1,
            clampToBounds: false
        )
        #expect(hit == at(1, 1), "滚动一列一行后同一点应命中 B2")

        // 2. 冻结列：冻结区内 X 轴不叠加滚动量，滚动区照常叠加
        let wide = makeSheet(grid: [
            ["0,0", "0,1", "0,2", "0,3"],
            ["1,0", "1,1", "1,2", "1,3"],
        ])
        let wideLayout = makeLayout(wide)
        let frozenW: CGFloat = 64
        hit = SheetSelectionModel.hitTest(
            layout: wideLayout, effFrozenRows: 0, effFrozenCols: 1,
            frozenDataW: frozenW, frozenDataH: 0, scrollDataX: 100, scrollDataY: 0,
            viewX: SheetFixture.headerWidth + 10, viewY: SheetFixture.headerHeight + 1,
            clampToBounds: false
        )
        #expect(hit?.column == 0, "冻结列忽略滚动量")
        hit = SheetSelectionModel.hitTest(
            layout: wideLayout, effFrozenRows: 0, effFrozenCols: 1,
            frozenDataW: frozenW, frozenDataH: 0, scrollDataX: 64, scrollDataY: 0,
            viewX: SheetFixture.headerWidth + frozenW + 1, viewY: SheetFixture.headerHeight + 1,
            clampToBounds: false
        )
        #expect(hit?.column == 2, "可滚动列叠加滚动量")

        // 3. 冻结行：8 行 × 20 = 160 的数据高度，scrollY = 60，滚动区应命中第 4 行
        let tall = makeSheet(grid: (0..<8).map { ["r\($0)"] })
        let tallLayout = makeLayout(tall)
        let frozenH: CGFloat = 20
        let scrollY: CGFloat = 60
        hit = SheetSelectionModel.hitTest(
            layout: tallLayout, effFrozenRows: 1, effFrozenCols: 0,
            frozenDataW: 0, frozenDataH: frozenH, scrollDataX: 0, scrollDataY: scrollY,
            viewX: SheetFixture.headerWidth + 1, viewY: SheetFixture.headerHeight + 1,
            clampToBounds: false
        )
        #expect(hit?.row == 0, "冻结行忽略滚动量")
        hit = SheetSelectionModel.hitTest(
            layout: tallLayout, effFrozenRows: 1, effFrozenCols: 0,
            frozenDataW: 0, frozenDataH: frozenH, scrollDataX: 0, scrollDataY: scrollY,
            viewX: SheetFixture.headerWidth + 1, viewY: SheetFixture.headerHeight + frozenH + 1,
            clampToBounds: false
        )
        #expect(hit?.row == 4, "可滚动行叠加滚动量后应命中第 4 行")

        // 4. 双轴冻结：交叉角两轴都不叠加滚动量
        let grid = (0..<10).map { row in (0..<8).map { "\(row),\($0)" } }
        let corner = makeSheet(grid: grid)
        let cornerLayout = makeLayout(corner)
        let frozenWidth: CGFloat = 64
        let frozenHeight: CGFloat = 40
        let scrollX: CGFloat = 128
        var cornerHit = SheetSelectionModel.hitTest(
            layout: cornerLayout, effFrozenRows: 2, effFrozenCols: 1,
            frozenDataW: frozenWidth, frozenDataH: frozenHeight, scrollDataX: scrollX, scrollDataY: 60,
            viewX: SheetFixture.headerWidth + frozenWidth - 1,
            viewY: SheetFixture.headerHeight + frozenHeight - 1,
            clampToBounds: false
        )
        #expect(cornerHit == at(1, 0), "交叉角锁定两轴")
        cornerHit = SheetSelectionModel.hitTest(
            layout: cornerLayout, effFrozenRows: 2, effFrozenCols: 1,
            frozenDataW: frozenWidth, frozenDataH: frozenHeight, scrollDataX: scrollX, scrollDataY: 60,
            viewX: SheetFixture.headerWidth + 1,
            viewY: SheetFixture.headerHeight + frozenHeight + 1,
            clampToBounds: false
        )
        #expect(cornerHit?.column == 0 && cornerHit?.row == 5, "冻结列 + 滚动行应命中第 5 行第 0 列")
        cornerHit = SheetSelectionModel.hitTest(
            layout: cornerLayout, effFrozenRows: 2, effFrozenCols: 1,
            frozenDataW: frozenWidth, frozenDataH: frozenHeight, scrollDataX: scrollX, scrollDataY: 60,
            viewX: SheetFixture.headerWidth + frozenWidth + 1,
            viewY: SheetFixture.headerHeight + 1,
            clampToBounds: false
        )
        #expect(cornerHit?.row == 0 && cornerHit?.column == 3, "冻结行 + 滚动列应命中第 0 行第 3 列")
        cornerHit = SheetSelectionModel.hitTest(
            layout: cornerLayout, effFrozenRows: 2, effFrozenCols: 1,
            frozenDataW: frozenWidth, frozenDataH: frozenHeight, scrollDataX: scrollX, scrollDataY: 60,
            viewX: SheetFixture.headerWidth + frozenWidth + 1,
            viewY: SheetFixture.headerHeight + frozenHeight + 1,
            clampToBounds: false
        )
        #expect(cornerHit == at(5, 3), "纯滚动区叠加两轴偏移")
    }

    @Test("XLSX 标头内不命中、越界按钳位开关返回 nil 或边缘格")
    func sheetSelectionHitTestClampAndHeaders() {
        let sheet = makeSheet(grid: [["0,0", "0,1"], ["1,0", "1,1"]])
        let layout = makeLayout(sheet)
        // 标头内：不产生单元格命中
        #expect(SheetSelectionModel.hitTest(
            layout: layout, effFrozenRows: 0, effFrozenCols: 0,
            frozenDataW: 0, frozenDataH: 0, scrollDataX: 0, scrollDataY: 0,
            viewX: SheetFixture.headerWidth - 1, viewY: SheetFixture.headerHeight + 1,
            clampToBounds: false
        ) == nil, "列标内不应命中单元格")
        #expect(SheetSelectionModel.hitTest(
            layout: layout, effFrozenRows: 0, effFrozenCols: 0,
            frozenDataW: 0, frozenDataH: 0, scrollDataX: 0, scrollDataY: 0,
            viewX: SheetFixture.headerWidth + 1, viewY: SheetFixture.headerHeight - 1,
            clampToBounds: false
        ) == nil, "行标内不应命中单元格")
        #expect(SheetSelectionModel.hitTest(
            layout: layout, effFrozenRows: 0, effFrozenCols: 0,
            frozenDataW: 0, frozenDataH: 0, scrollDataX: 0, scrollDataY: 0,
            viewX: SheetFixture.headerWidth + 1, viewY: SheetFixture.headerHeight + 1,
            clampToBounds: false
        ) != nil, "正文内应命中单元格")

        // 越过右下边界：不钳位返回 nil，钳位贴到末行末列
        let farX = SheetFixture.headerWidth + 64 * 2 + 50
        let farY = SheetFixture.headerHeight + 20 * 2 + 50
        #expect(SheetSelectionModel.hitTest(
            layout: layout, effFrozenRows: 0, effFrozenCols: 0,
            frozenDataW: 0, frozenDataH: 0, scrollDataX: 0, scrollDataY: 0,
            viewX: farX, viewY: farY, clampToBounds: false
        ) == nil, "越界且不钳位应返回 nil")
        #expect(SheetSelectionModel.hitTest(
            layout: layout, effFrozenRows: 0, effFrozenCols: 0,
            frozenDataW: 0, frozenDataH: 0, scrollDataX: 0, scrollDataY: 0,
            viewX: farX, viewY: farY, clampToBounds: true
        ) == at(1, 1), "越界钳位应贴到末行末列")

        // 滚动量大于表格尺寸时数据坐标同样越界，钳位后仍落在合法末格（绝不返回负索引）
        #expect(SheetSelectionModel.hitTest(
            layout: layout, effFrozenRows: 0, effFrozenCols: 0,
            frozenDataW: 0, frozenDataH: 0, scrollDataX: 200, scrollDataY: 200,
            viewX: SheetFixture.headerWidth + 1, viewY: SheetFixture.headerHeight + 1,
            clampToBounds: true
        ) == at(1, 1), "过度滚动钳位后应落在末格")
        #expect(SheetSelectionModel.hitTest(
            layout: layout, effFrozenRows: 0, effFrozenCols: 0,
            frozenDataW: 0, frozenDataH: 0, scrollDataX: 200, scrollDataY: 200,
            viewX: SheetFixture.headerWidth + 1, viewY: SheetFixture.headerHeight + 1,
            clampToBounds: false
        ) == nil, "过度滚动且不钳位应返回 nil")
        #expect(SheetSelectionModel.hitTest(
            layout: layout, effFrozenRows: 0, effFrozenCols: 0,
            frozenDataW: 0, frozenDataH: 0, scrollDataX: 0, scrollDataY: 0,
            viewX: SheetFixture.headerWidth, viewY: SheetFixture.headerHeight,
            clampToBounds: true
        ) == at(0, 0), "正文原点是首格")

        // 拖拽入标头/左上方视口外时，clampToBounds=true 收敛到当前可见左/上缘
        #expect(SheetSelectionModel.hitTest(
            layout: layout, effFrozenRows: 0, effFrozenCols: 0,
            frozenDataW: 0, frozenDataH: 0, scrollDataX: 0, scrollDataY: 0,
            viewX: SheetFixture.headerWidth - 12, viewY: SheetFixture.headerHeight + 5,
            clampToBounds: true
        ) == at(0, 0), "拖入行标头钳位后应落在列 0")
        #expect(SheetSelectionModel.hitTest(
            layout: layout, effFrozenRows: 0, effFrozenCols: 0,
            frozenDataW: 0, frozenDataH: 0, scrollDataX: 64, scrollDataY: 20,
            viewX: SheetFixture.headerWidth - 10, viewY: SheetFixture.headerHeight - 10,
            clampToBounds: true
        ) == at(1, 1), "滚动后拖入标头钳位应落在左上可见格")
        #expect(SheetSelectionModel.hitTest(
            layout: layout, effFrozenRows: 1, effFrozenCols: 1,
            frozenDataW: 64, frozenDataH: 20, scrollDataX: 64, scrollDataY: 20,
            viewX: SheetFixture.headerWidth - 10, viewY: SheetFixture.headerHeight - 10,
            clampToBounds: true
        ) == at(0, 0), "冻结角下拖入标头钳位应落在 (0, 0)")

        // NaN 触点直接拒绝，避免污染二分结果
        #expect(SheetSelectionModel.hitTest(
            layout: layout, effFrozenRows: 0, effFrozenCols: 0,
            frozenDataW: 0, frozenDataH: 0, scrollDataX: 0, scrollDataY: 0,
            viewX: CGFloat.nan, viewY: SheetFixture.headerHeight + 1,
            clampToBounds: true
        ) == nil, "NaN 触点不应命中单元格")
    }

    @Test("XLSX 全选范围：空表返回 nil，非空表覆盖首个到末个可见格")
    func sheetSelectionAllRangeRequiresText() {
        // 全空表
        let blank = makeSheet(grid: [[nil, nil], [nil, nil]])
        #expect(SheetSelectionModel.allRange(sheet: blank, layout: makeLayout(blank)) == nil)
        // 只有空白文本的单元格也算空
        let spaces = makeSheet(grid: [["", ""]])
        #expect(SheetSelectionModel.allRange(sheet: spaces, layout: makeLayout(spaces)) == nil)

        // 有文本的表：覆盖全部可见行列
        let sheet = makeSheet(grid: [["A1", "B1", "C1"], ["A2", "B2", "C2"]])
        let layout = makeLayout(sheet)
        let all = SheetSelectionModel.allRange(sheet: sheet, layout: layout)
        #expect(all != nil)
        #expect(all?.startRow == 0)
        #expect(all?.startColumn == 0)
        #expect(all?.endRow == 1)
        #expect(all?.endColumn == 2)
        #expect(
            SheetSelectionModel.selectedText(sheet: sheet, layout: layout, selection: all)
                == "A1\tB1\tC1\nA2\tB2\tC2"
        )

        // 末列/末行隐藏时全选跟随可见范围收缩
        let source = makeSheet(grid: [["A1", "B1", "C1"], ["A2", "B2", "C2"]])
        var widths = source.columnWidths
        widths[2] = 0
        var heights = source.rowHeights
        heights[1] = 0
        let hiddenTail = OfficeSheet(
            name: source.name,
            rowHeights: heights,
            columnWidths: widths,
            cells: source.cells,
            merges: source.merges
        )
        let hiddenLayout = makeLayout(hiddenTail)
        let shrunk = SheetSelectionModel.allRange(sheet: hiddenTail, layout: hiddenLayout)
        #expect(shrunk != nil)
        #expect(shrunk?.endRow == 0, "隐藏末行应排除")
        #expect(shrunk?.endColumn == 1, "隐藏末列应排除")
        #expect(
            SheetSelectionModel.selectedText(sheet: hiddenTail, layout: hiddenLayout, selection: shrunk) == "A1\tB1"
        )
    }

    @Test("XLSX 行列标头联动高亮判定")
    func sheetSelectionHeaderLinkage() {
        let sheet = makeSheet(grid: [
            ["a", "b", "c", "d"],
            ["e", "f", "g", "h"],
            ["i", "j", "k", "l"],
        ])
        let selection = SheetSelectionModel.resolve(sheet: sheet, anchor: at(1, 1), focus: at(2, 2))
        #expect(selection != nil)
        guard let selection else { return }

        #expect(selection.containsRow(1))
        #expect(selection.containsRow(2))
        #expect(!selection.containsRow(0))
        #expect(!selection.containsRow(3))

        #expect(selection.containsColumn(1))
        #expect(selection.containsColumn(2))
        #expect(!selection.containsColumn(0))
        #expect(!selection.containsColumn(3))

        #expect(selection.covers(row: 1, column: 2))
        #expect(!selection.covers(row: 0, column: 0))
    }

    @Test("XLSX 选区矩形几何与越界安全返回")
    func sheetSelectionDocRectGeometry() {
        let sheet = makeSheet(grid: [["a", "b", "c"], ["d", "e", "f"], ["g", "h", "i"]])
        let layout = makeLayout(sheet)
        let selection = SheetSelectionModel.resolve(sheet: sheet, anchor: at(1, 1), focus: at(2, 2))
        let rect = SheetSelectionModel.docRect(layout: layout, selection: selection)
        #expect(rect != nil)
        #expect(Int(rect!.left.rounded()) == Int((SheetFixture.headerWidth + 64).rounded()))
        #expect(Int(rect!.right.rounded()) == Int((SheetFixture.headerWidth + 192).rounded()))
        #expect(Int(rect!.top.rounded()) == Int((SheetFixture.headerHeight + 20).rounded()))
        #expect(Int(rect!.bottom.rounded()) == Int((SheetFixture.headerHeight + 60).rounded()))
        #expect(Int(rect!.width.rounded()) == 128)
        #expect(Int(rect!.height.rounded()) == 40)
        #expect(!rect!.isEmpty)

        // 越界选区安全返回 nil，避免渲染层数组越界
        let outOfBounds = SheetSelectionModel.SheetSelection(
            anchor: at(0, 0), focus: at(9, 9),
            startRow: 0, startColumn: 0, endRow: 9, endColumn: 9
        )
        #expect(SheetSelectionModel.docRect(layout: layout, selection: outOfBounds) == nil)
        #expect(SheetSelectionModel.docRect(layout: layout, selection: nil) == nil)

        // 合并区域的矩形与选区矩形使用同一套坐标系
        let mergeRange = OfficeCellRange(startRow: 0, startColumn: 0, endRow: 0, endColumn: 1)
        let merged = makeSheet(grid: [["a", "b"], ["c", "d"]], merges: [mergeRange])
        let mergeRect = SheetSelectionModel.mergeDocRect(layout: makeLayout(merged), merge: mergeRange)
        #expect(mergeRect != nil)
        #expect(Int(mergeRect!.width.rounded()) == 128)
        #expect(SheetSelectionModel.mergeDocRect(
            layout: makeLayout(merged),
            merge: OfficeCellRange(startRow: 0, startColumn: 0, endRow: 9, endColumn: 9)
        ) == nil)
    }

    @Test("XLSX 选区绘制：无冻结单象限与冻结四象限裁剪")
    func sheetSelectionDrawQuadrants() throws {
        // 未冻结：整块选区只产生 1 个绘制片段
        let plain = makeSheet(grid: [["A1", "B1"], ["A2", "B2"]])
        let plainViewport = CGRect(x: 0, y: 0, width: 300, height: 200)
        let plainPlan = CGElementDrawer.sheetFreezePlan(sheet: plain, viewport: plainViewport)
        #expect(!plainPlan.hasFrozen)
        let plainLayout = CGElementDrawer.sheetSelectionLayout(sheet: plain)
        let plainSelection = SheetSelectionModel.resolve(sheet: plain, anchor: at(0, 0), focus: at(1, 1))
        let plainRect = SheetSelectionModel.docRect(layout: plainLayout, selection: plainSelection)
        #expect(plainRect != nil)
        let plainPieces = CGElementDrawer.clipSheetHighlight(
            CGRect(x: plainRect!.left, y: plainRect!.top, width: plainRect!.width, height: plainRect!.height),
            plan: plainPlan,
            rowCount: 2,
            columnCount: 2
        )
        #expect(plainPieces.count == 1)

        // 冻结行列：跨冻结边界的选区拆到多个象限，且每片裁剪区域互不重叠
        let frozen = try freezeSheet(frozenRows: 1, frozenColumns: 1)
        let viewport = CGRect(x: 0, y: 0, width: 320, height: 220)
        let plan = CGElementDrawer.sheetFreezePlan(sheet: frozen, viewport: viewport)
        #expect(plan.effectiveRows == 1)
        #expect(plan.effectiveColumns == 1)
        let layout = CGElementDrawer.sheetSelectionLayout(sheet: frozen)
        // 横跨交叉角与滚动区的选区
        let selection = SheetSelectionModel.resolve(sheet: frozen, anchor: at(0, 0), focus: at(2, 2))
        let rect = SheetSelectionModel.docRect(layout: layout, selection: selection)
        #expect(rect != nil)
        let pieces = CGElementDrawer.clipSheetHighlight(
            CGRect(x: rect!.left, y: rect!.top, width: rect!.width, height: rect!.height),
            plan: plan,
            rowCount: 6,
            columnCount: 6
        )
        #expect(pieces.count == 4, "跨四象限的选区应拆成 4 片，实际 \(pieces.count)")
        for piece in pieces {
            #expect(!piece.clip.isEmpty)
        }

        // 离屏绘制：带选区的输出必须包含选区填充色（说明选区真的画上去了）
        let colorSpace = CGColorSpace(name: CGColorSpace.sRGB) ?? CGColorSpaceCreateDeviceRGB()
        let width = 300, height = 240
        var bytes = [UInt8](repeating: 255, count: width * height * 4)
        let ctx = try #require(CGContext(
            data: &bytes,
            width: width,
            height: height,
            bitsPerComponent: 8,
            bytesPerRow: width * 4,
            space: colorSpace,
            bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue
        ))
        ctx.translateBy(x: 0, y: CGFloat(height))
        ctx.scaleBy(x: 1, y: -1)
        let drawViewport = CGRect(x: 0, y: 0, width: 300, height: 240)
        let drawSelection = SheetSelectionModel.resolve(sheet: frozen, anchor: at(0, 0), focus: at(1, 1))
        CGElementDrawer.drawSpreadsheet(
            in: ctx,
            sheet: frozen,
            styles: [],
            viewport: drawViewport,
            selection: drawSelection
        )
        // 选区填充为 15% Google Blue 与白底混合，取一点确认偏蓝
        func pixelRGBA(_ x: Int, _ y: Int) -> (UInt8, UInt8, UInt8, UInt8) {
            let idx = (y * width + x) * 4
            return (bytes[idx], bytes[idx + 1], bytes[idx + 2], bytes[idx + 3])
        }
        let inside = pixelRGBA(82, 32)
        #expect(Int(inside.2) > Int(inside.0), "选区内部应偏蓝，实际 \(inside)")
        // 标头联动高亮应为 #D2E3FC
        let headerPixel = pixelRGBA(120, 10)
        #expect(
            abs(Int(headerPixel.0) - 0xd2) <= 8
                && abs(Int(headerPixel.1) - 0xe3) <= 8
                && abs(Int(headerPixel.2) - 0xfc) <= 8,
            "选中列的标头应为 #D2E3FC，实际 \(headerPixel)"
        )
        // 未选中列（第 2 列起）仍为常规底色 #F3F5F7
        let plainHeaderPixel = pixelRGBA(220, 10)
        #expect(
            abs(Int(plainHeaderPixel.0) - 0xf3) <= 8
                && abs(Int(plainHeaderPixel.1) - 0xf5) <= 8
                && abs(Int(plainHeaderPixel.2) - 0xf7) <= 8,
            "未选中列的标头应保持 #F3F5F7，实际 \(plainHeaderPixel)"
        )
    }

    @Test("XLSX 模型布局复用 core 预计算几何，脏 rowStarts 回退重算")
    func sheetSelectionLayoutReusesGeometry() {
        // 预计算几何：模型必须复用它（前缀和等于 core 值 + 标头 gutter）
        let geometry = OfficeSheetGeometry(
            columnOffsets: [0, 70, 130],
            rowOffsets: [0, 25, 55],
            columnVisible: [0, 1],
            rowVisible: [0, 1],
            rowStarts: [0, 1, 2],
            mergedCells: [false, false],
            mergeRects: []
        )
        let sheet = OfficeSheet(
            name: "Geom",
            rowHeights: [25, 30],
            columnWidths: [70, 60],
            cells: [
                OfficeCell(row: 0, column: 0, text: "A"),
                OfficeCell(row: 1, column: 1, text: "B"),
            ],
            geometry: geometry
        )
        let layout = CGElementDrawer.sheetSelectionLayout(sheet: sheet)
        #expect(layout.columns.offsets == [42, 112, 172])
        #expect(layout.rows.offsets == [22, 47, 77])
        // 与绘制层前缀和逐像素一致
        let (colOffsets, rowOffsets, _, _) = CGElementDrawer.computeSheetExtents(sheet)
        #expect(layout.columns.offsets == colOffsets)
        #expect(layout.rows.offsets == rowOffsets)
        #expect(SheetSelectionModel.cellTextAt(sheet: sheet, layout: layout, row: 1, column: 1) == "B")

        // 脏 rowStarts（长度不符）→ 现场重算，定位仍然正确
        let dirty = OfficeSheetGeometry(
            columnOffsets: [0, 70, 130],
            rowOffsets: [0, 25, 55],
            columnVisible: [0, 1],
            rowVisible: [0, 1],
            rowStarts: [],
            mergedCells: [],
            mergeRects: []
        )
        let dirtySheet = OfficeSheet(
            name: "Dirty",
            rowHeights: [25, 30],
            columnWidths: [70, 60],
            cells: [
                OfficeCell(row: 0, column: 0, text: "A"),
                OfficeCell(row: 1, column: 1, text: "B"),
            ],
            geometry: dirty
        )
        let dirtyLayout = CGElementDrawer.sheetSelectionLayout(sheet: dirtySheet)
        #expect(dirtyLayout.rowStarts == [0, 1, 2])
        #expect(SheetSelectionModel.cellTextAt(sheet: dirtySheet, layout: dirtyLayout, row: 1, column: 1) == "B")

        // 无 geometry：按尺寸现场累加，与 computeSheetExtents 回退分支一致
        let bare = makeSheet(grid: [["a", "b"], ["c", "d"]])
        let bareLayout = CGElementDrawer.sheetSelectionLayout(sheet: bare)
        let (bareCols, bareRows, _, _) = CGElementDrawer.computeSheetExtents(bare)
        #expect(bareLayout.columns.offsets == bareCols)
        #expect(bareLayout.rows.offsets == bareRows)
    }

    @Test("XLSX 鼠标点换算为视图数据坐标并命中单元格")
    @MainActor
    func sheetSelectionMousePointConversion() throws {
        // 用真实文档驱动视图，端到端校验「屏幕点 → 文档点 → 模型视图数据坐标」这条链路
        let doc = try JSONDecoder().decode(OfficeDocument.self, from: """
        {
            "schemaVersion": 9,
            "kind": "XLSX",
            "sheets": [
                {
                    "name": "Grid",
                    "rowHeights": [20, 20, 20],
                    "columnWidths": [64, 64, 64],
                    "cells": [
                        {"row": 0, "column": 0, "text": "0,0", "style": 0, "numeric": false},
                        {"row": 1, "column": 1, "text": "1,1", "style": 0, "numeric": false},
                        {"row": 2, "column": 2, "text": "2,2", "style": 0, "numeric": false}
                    ],
                    "merges": []
                }
            ],
            "cellStyles": []
        }
        """.data(using: .utf8)!)

        let view = OfficeDocumentNSView()
        view.frame = CGRect(x: 0, y: 0, width: 400, height: 300)
        view.setDocument(doc)
        view.currentPage = 1
        view.zoom = 1

        // 未滚动：可见窗口原点为 (0,0)，视图点 (43,23) 落在 A1
        let unrolled = CGRect(x: 0, y: 0, width: 400, height: 300)
        #expect(view.sheetCell(atViewPoint: CGPoint(x: 43, y: 23), docVisible: unrolled, clampToBounds: false) == at(0, 0))
        // 视图点 (150,45) 落在第 1 列第 1 行 = B2
        #expect(view.sheetCell(atViewPoint: CGPoint(x: 150, y: 45), docVisible: unrolled, clampToBounds: false) == at(1, 1))

        // 标头区域内不命中
        #expect(view.sheetCell(atViewPoint: CGPoint(x: 20, y: 23), docVisible: unrolled, clampToBounds: false) == nil)
        #expect(view.sheetCell(atViewPoint: CGPoint(x: 43, y: 10), docVisible: unrolled, clampToBounds: false) == nil)

        // 滚动 (128,40)：可见窗口原点即滚动量，视图点 (171,63) 减去原点得模型坐标 (43,23)，
        // 叠加滚动量后落在 C3；作为对照，在未滚动窗口中同一视口相对点 (43,23) 对应 A1
        let scrolled = CGRect(x: 128, y: 40, width: 400, height: 300)
        #expect(view.sheetCell(atViewPoint: CGPoint(x: 171, y: 63), docVisible: scrolled, clampToBounds: false) == at(2, 2))
        #expect(view.sheetCell(atViewPoint: CGPoint(x: 43, y: 23), docVisible: unrolled, clampToBounds: false) == at(0, 0))

        // 冻结窗格端到端命中：冻结 1 行 1 列并在缩放 2 倍、滚动 (128,40) 下校验四象限
        let frozenDoc = try JSONDecoder().decode(OfficeDocument.self, from: """
        {
            "schemaVersion": 9,
            "kind": "XLSX",
            "sheets": [
                {
                    "name": "Frozen",
                    "rowHeights": [20, 20, 20, 20, 20],
                    "columnWidths": [64, 64, 64, 64, 64],
                    "frozenRows": 1,
                    "frozenColumns": 1,
                    "cells": [],
                    "merges": []
                }
            ],
            "cellStyles": []
        }
        """.data(using: .utf8)!)
        let frozenView = OfficeDocumentNSView()
        frozenView.frame = CGRect(x: 0, y: 0, width: 800, height: 600)
        frozenView.setDocument(frozenDoc)
        frozenView.currentPage = 1
        frozenView.zoom = 2
        let frozenScrolled = CGRect(x: 128, y: 40, width: 400, height: 300)
        // 交叉角（视图相对坐标落在表头之后、冻结区之内）：(128+50)*2, (40+30)*2 → (0,0)
        #expect(frozenView.sheetCell(
            atViewPoint: CGPoint(x: (128 + 50) * 2, y: (40 + 30) * 2),
            docVisible: frozenScrolled, clampToBounds: false
        ) == at(0, 0))
        // 冻结列 + 滚动行：(128+50)*2, (40+22+20+5)*2 → 列 0, 行 (25+40)/20 = 3
        #expect(frozenView.sheetCell(
            atViewPoint: CGPoint(x: (128 + 50) * 2, y: (40 + 47) * 2),
            docVisible: frozenScrolled, clampToBounds: false
        ) == at(3, 0))
        // 冻结行 + 滚动列：(128+42+64+10)*2, (40+30)*2 → 行 0, 列 (74+128)/64 = 3
        #expect(frozenView.sheetCell(
            atViewPoint: CGPoint(x: (128 + 116) * 2, y: (40 + 30) * 2),
            docVisible: frozenScrolled, clampToBounds: false
        ) == at(0, 3))
        // 拖拽入标头时 clampToBounds=true 收敛到冻结交叉角 (0, 0)
        #expect(frozenView.sheetCell(
            atViewPoint: CGPoint(x: (128 + 10) * 2, y: (40 + 5) * 2),
            docVisible: frozenScrolled, clampToBounds: true
        ) == at(0, 0))

        // 缩放 2：屏幕点翻倍后换算结果不变
        view.zoom = 2
        #expect(view.sheetCell(atViewPoint: CGPoint(x: 86, y: 46), docVisible: unrolled, clampToBounds: false) == at(0, 0))

        // 越界：不钳位返回 nil，钳位贴到末格
        view.zoom = 1
        #expect(view.sheetCell(
            atViewPoint: CGPoint(x: 900, y: 900), docVisible: unrolled, clampToBounds: false
        ) == nil)
        #expect(view.sheetCell(
            atViewPoint: CGPoint(x: 900, y: 900), docVisible: unrolled, clampToBounds: true
        ) == at(2, 2))

        // 负原点可见矩形（居中留白、外边距或透明顶栏安全区内边距导致 visibleRect.minX/minY < 0）：
        // 表格实际仍从画布 (0, 0) 绘制，命中换算必须按 clamp 后的 plan.scrollX/Y = 0 对齐，不能把负偏移加回触点
        let negativeOrigin = CGRect(x: -200, y: -150, width: 1000, height: 600)
        #expect(view.sheetCell(atViewPoint: CGPoint(x: 43, y: 23), docVisible: negativeOrigin, clampToBounds: false) == at(0, 0))
        #expect(view.sheetCell(atViewPoint: CGPoint(x: 150, y: 45), docVisible: negativeOrigin, clampToBounds: false) == at(1, 1))
        #expect(view.sheetCell(atViewPoint: CGPoint(x: 20, y: 23), docVisible: negativeOrigin, clampToBounds: false) == nil)
        #expect(frozenView.sheetCell(atViewPoint: CGPoint(x: 50 * 2, y: 30 * 2), docVisible: negativeOrigin, clampToBounds: false) == at(0, 0))

        // 真实 NSScrollView 带顶部统一工具栏内边距 (contentInsets.top = 52)：
        // 未传 overrideDocVisible 时也应扣掉 top inset，在静止和向下滚动后都能精确命中目标格
        let scrollView = NSScrollView(frame: CGRect(x: 0, y: 0, width: 400, height: 300))
        scrollView.automaticallyAdjustsContentInsets = false
        scrollView.contentInsets = NSEdgeInsets(top: 52, left: 0, bottom: 0, right: 0)
        let hostView = OfficeDocumentNSView(frame: CGRect(x: 0, y: 0, width: 400, height: 500))
        hostView.setDocument(doc)
        hostView.currentPage = 1
        hostView.zoom = 1
        scrollView.documentView = hostView
        scrollView.contentView.scroll(to: NSPoint(x: 0, y: -52))
        scrollView.reflectScrolledClipView(scrollView.contentView)
        #expect(hostView.sheetCell(atViewPoint: CGPoint(x: 43, y: 23), clampToBounds: false) == at(0, 0))
        #expect(hostView.sheetCell(atViewPoint: CGPoint(x: 150, y: 45), clampToBounds: false) == at(1, 1))

        // 向下滚动 40pt（origin.y = -12，有效滚动量 scrollY = 40）：
        // 视口左上正文 (43, 63) 处于第 2 行第 0 列，必须命中 (2, 0)
        scrollView.contentView.scroll(to: NSPoint(x: 0, y: -12))
        scrollView.reflectScrolledClipView(scrollView.contentView)
        #expect(hostView.sheetCell(atViewPoint: CGPoint(x: 43, y: 63), clampToBounds: false) == at(2, 0))

        // 视图状态默认值：无文档时不构造布局
        let empty = OfficeDocumentNSView()
        #expect(empty.currentSheetLayout() == nil)
        #expect(empty.sheetSelection == nil)
        #expect(!empty.hasAnySelection)
        #expect(empty.performSelectAll() == false)
        #expect(empty.currentSelectedText() == "")
    }

    @Test("XLSX 选区状态在切表、换文档与清除时被丢弃")
    @MainActor
    func sheetSelectionStateLifecycle() throws {
        let view = OfficeDocumentNSView()
        let sheetA = makeSheet(grid: [["A1", "B1"], ["A2", "B2"]])
        let selection = SheetSelectionModel.resolve(sheet: sheetA, anchor: at(0, 0), focus: at(1, 1))

        // 两页文档：切表才有意义
        let twoSheetDoc = try JSONDecoder().decode(OfficeDocument.self, from: """
        {
            "schemaVersion": 9,
            "kind": "XLSX",
            "sheets": [
                {"name": "S1", "rowHeights": [20, 20], "columnWidths": [64, 64],
                 "cells": [{"row": 0, "column": 0, "text": "A1", "style": 0, "numeric": false}], "merges": []},
                {"name": "S2", "rowHeights": [20], "columnWidths": [64],
                 "cells": [{"row": 0, "column": 0, "text": "B1", "style": 0, "numeric": false}], "merges": []}
            ],
            "cellStyles": []
        }
        """.data(using: .utf8)!)
        view.setDocument(twoSheetDoc)
        view.currentPage = 1

        // 清除选区
        view.sheetSelection = selection
        #expect(view.hasAnySelection)
        view.performClearSelection()
        #expect(view.sheetSelection == nil)

        // 切换工作表：必须丢弃（单元格坐标只对原表有意义）
        view.sheetSelection = selection
        view.currentPage = 2
        #expect(view.sheetSelection == nil)

        // setDocument 换文档：必须丢弃
        let otherDoc = try JSONDecoder().decode(OfficeDocument.self, from: """
        {
            "schemaVersion": 9,
            "kind": "XLSX",
            "sheets": [
                {"name": "Other", "rowHeights": [20], "columnWidths": [64],
                 "cells": [{"row": 0, "column": 0, "text": "Z", "style": 0, "numeric": false}], "merges": []}
            ],
            "cellStyles": []
        }
        """.data(using: .utf8)!)
        view.sheetSelection = selection
        view.setDocument(otherDoc)
        #expect(view.sheetSelection == nil)

        // teardown 释放全部选区状态
        view.sheetSelection = selection
        view.teardown()
        #expect(view.sheetSelection == nil)

        // 非 XLSX 文档：DOCX 的字段区选语义不受影响，hasAnySelection 不把表格选区算进去
        let docxDoc = try JSONDecoder().decode(OfficeDocument.self, from: """
        {"schemaVersion": 9, "kind": "DOCX", "blocks": [], "pages": []}
        """.data(using: .utf8)!)
        let docxView = OfficeDocumentNSView()
        docxView.setDocument(docxDoc)
        docxView.sheetSelection = selection
        #expect(!docxView.hasAnySelection, "DOCX 下不应把表格选区算作有效选区")
    }

    @Test("XLSX 选中文本与 TSV 写入剪贴板")
    @MainActor
    func sheetSelectionClipboardBridge() throws {
        let controller = OfficePreviewController()
        let view = OfficeDocumentNSView()
        controller.attachCanvas(view)

        // 未挂载文档时 5 个 API 全部安全降级
        #expect(controller.selectAll() == false)
        #expect(controller.hasSelection() == false)
        #expect(controller.getSelectedText() == "")
        #expect(controller.copySelection() == false)
        controller.clearSelection()

        // 注入表格文档后 API 走 XLSX 分支
        let sheet = makeSheet(grid: [["A1", "B1"], ["A2", "B2"]])
        let doc = try JSONDecoder().decode(OfficeDocument.self, from: """
        {
            "schemaVersion": 9,
            "kind": "XLSX",
            "sheets": [
                {
                    "name": "Sheet1",
                    "rowHeights": [20, 20],
                    "columnWidths": [64, 64],
                    "cells": [
                        {"row": 0, "column": 0, "text": "A1", "style": 0, "numeric": false},
                        {"row": 0, "column": 1, "text": "B1", "style": 0, "numeric": false},
                        {"row": 1, "column": 0, "text": "A2", "style": 0, "numeric": false},
                        {"row": 1, "column": 1, "text": "B2", "style": 0, "numeric": false}
                    ],
                    "merges": []
                }
            ],
            "cellStyles": []
        }
        """.data(using: .utf8)!)
        view.setDocument(doc)
        view.currentPage = 1
        #expect(view.currentSheetLayout() != nil)

        // 全选 → 整表 TSV
        #expect(controller.selectAll())
        #expect(controller.hasSelection())
        #expect(controller.getSelectedText() == "A1\tB1\nA2\tB2")

        // 复制 → 写入剪贴板并保持选区
        #expect(controller.copySelection())
        #expect(NSPasteboard.general.string(forType: .string) == "A1\tB1\nA2\tB2")
        #expect(controller.hasSelection(), "复制不应清除选区")

        // 单格 TSV 语义：选中一个单元格输出裸文本（无制表符）
        let single = SheetSelectionModel.resolve(sheet: sheet, anchor: at(0, 0), focus: at(0, 0))
        #expect(
            SheetSelectionModel.selectedText(sheet: sheet, layout: view.currentSheetLayout(), selection: single) == "A1"
        )

        // 清除后 API 回到空态
        controller.clearSelection()
        #expect(!controller.hasSelection())
        #expect(controller.getSelectedText() == "")

        // 换一份空白表：全选必须失败（对应 Java allRange 返回 null 的语义）
        let blankDoc = try JSONDecoder().decode(OfficeDocument.self, from: """
        {
            "schemaVersion": 9,
            "kind": "XLSX",
            "sheets": [
                {"name": "Blank", "rowHeights": [20, 20], "columnWidths": [64, 64], "cells": [], "merges": []}
            ],
            "cellStyles": []
        }
        """.data(using: .utf8)!)
        view.setDocument(blankDoc)
        #expect(!view.canSelectAll(), "空白表不应允许全选")
        #expect(!controller.selectAll())
        #expect(controller.getSelectedText() == "")
    }

    private func createTestCGImage(width: Int, height: Int) -> CGImage {
        let colorSpace = CGColorSpace(name: CGColorSpace.sRGB) ?? CGColorSpaceCreateDeviceRGB()
        var bytes = [UInt8](repeating: 128, count: width * height * 4)
        let ctx = CGContext(
            data: &bytes,
            width: width,
            height: height,
            bitsPerComponent: 8,
            bytesPerRow: width * 4,
            space: colorSpace,
            bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue
        )!
        return ctx.makeImage()!
    }

    // MARK: - XLSX 工作表浮动图片与形状

    /// 1×1 纯蓝 PNG（69 字节，sRGB、无透明通道）。
    ///
    /// 手写字节而不是引用样例文件：测试必须能在任意机器上确定性地产出同一张图，
    /// 并且「整块蓝色」是像素断言最容易区分于白底与灰色占位块的信号。
    private static let onePixelBluePNG: [UInt8] = [
        0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a,
        0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44, 0x52,
        0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01,
        0x08, 0x02, 0x00, 0x00, 0x00, 0x90, 0x77, 0x53,
        0xde, 0x00, 0x00, 0x00, 0x0c, 0x49, 0x44, 0x41,
        0x54, 0x78, 0xda, 0x63, 0x60, 0x60, 0xf8, 0x0f,
        0x00, 0x01, 0x03, 0x01, 0x00, 0x36, 0x74, 0x11,
        0x40, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4e,
        0x44, 0xae, 0x42, 0x60, 0x82,
    ]

    /// 用 store-only（不压缩）方式把若干部件打成 ZIP，供测试构造最小 OOXML 包。
    ///
    /// 只用 stored 是因为无损压缩器需要 deflate 实现；core 的 ZIP 读取对两种方式一视同仁，
    /// 而 store-only 的中央目录与本地头只需几十行字节计算，测试因此无需引入第三方依赖。
    private func makeStoredZip(entries: [(name: String, data: Data)]) -> Data {
        var local = Data()
        var central = Data()
        var centralSize = 0
        // 固定的 DOS 时间戳（2020-01-01 00:00:00）让同一份固件产出字节级一致的文件
        let dosTime: UInt16 = 0
        let dosDate: UInt16 = 0x5021
        for entry in entries {
            let nameBytes = Array(entry.name.utf8)
            let crc = Self.crc32(entry.data)
            let size = UInt32(entry.data.count)
            let offset = UInt32(local.count)

            // 本地文件头：签名 + 版本 + 标志 + 压缩法(0=stored) + 时间/日期 + CRC + 尺寸 + 文件名长度
            var header = Data()
            header.append(contentsOf: [0x50, 0x4b, 0x03, 0x04])
            header.append(contentsOf: [20, 0])                    // version needed
            header.append(contentsOf: [0, 0])                     // flags
            header.append(contentsOf: [0, 0])                     // method = stored
            header.append(littleEndian: dosTime)
            header.append(littleEndian: dosDate)
            header.append(littleEndian: crc)
            header.append(littleEndian: size)
            header.append(littleEndian: size)
            header.append(littleEndian: UInt16(nameBytes.count))
            header.append(contentsOf: [0, 0])                     // extra field length
            header.append(contentsOf: nameBytes)
            local.append(header)
            local.append(entry.data)

            // 中央目录项：与本地头字段对齐，额外记录本地头偏移
            var record = Data()
            record.append(contentsOf: [0x50, 0x4b, 0x01, 0x02])
            record.append(contentsOf: [20, 0])                    // version made by
            record.append(contentsOf: [20, 0])                    // version needed
            record.append(contentsOf: [0, 0])                     // flags
            record.append(contentsOf: [0, 0])                     // method = stored
            record.append(littleEndian: dosTime)
            record.append(littleEndian: dosDate)
            record.append(littleEndian: crc)
            record.append(littleEndian: size)
            record.append(littleEndian: size)
            record.append(littleEndian: UInt16(nameBytes.count))
            record.append(contentsOf: [0, 0])                     // extra field length
            record.append(contentsOf: [0, 0])                     // comment length
            record.append(contentsOf: [0, 0])                     // disk number
            record.append(contentsOf: [0, 0])                     // internal attrs
            record.append(contentsOf: [0, 0, 0, 0])               // external attrs
            record.append(littleEndian: offset)
            record.append(contentsOf: nameBytes)
            central.append(record)
            centralSize += record.count
        }
        var output = local
        output.append(central)
        let centralOffset = UInt32(local.count)
        var end = Data()
        end.append(contentsOf: [0x50, 0x4b, 0x05, 0x06])
        end.append(contentsOf: [0, 0])                            // disk number
        end.append(contentsOf: [0, 0])                            // central dir start disk
        end.append(littleEndian: UInt16(entries.count))
        end.append(littleEndian: UInt16(entries.count))
        end.append(littleEndian: UInt32(centralSize))
        end.append(littleEndian: centralOffset)
        end.append(contentsOf: [0, 0])                            // comment length
        output.append(end)
        return output
    }

    private static func crc32(_ data: Data) -> UInt32 {
        var table = [UInt32](repeating: 0, count: 256)
        for i in 0..<256 {
            var value = UInt32(i)
            for _ in 0..<8 {
                value = (value & 1) != 0 ? (value >> 1) ^ 0xEDB8_8320 : value >> 1
            }
            table[i] = value
        }
        var crc: UInt32 = 0xFFFF_FFFF
        for byte in data {
            crc = table[Int((crc ^ UInt32(byte)) & 0xff)] ^ (crc >> 8)
        }
        return crc ^ 0xFFFF_FFFF
    }

    /// 构造最小但真实的 XLSX 包：`<xdr:pic>` 引用 `xl/media/image1.png`，另一个 `<xdr:sp>` 纯红填充。
    ///
    /// 两个锚点都落在 A1 附近（列 0/行 0 起），因此首屏视口必然可见；
    /// 图片部件真的存在于包内，所以 `ImageStore.loadSync` 会走真实的读包 + ImageIO 解码路径。
    private func makeXlsxWithPictureAndShape() -> Data {
        func rels(_ items: [(String, String, String)]) -> String {
            let body = items
                .map { #"<Relationship Id="\#($0.0)" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/\#($0.1)" Target="\#($0.2)"/>"# }
                .joined()
            return #"<?xml version="1.0" encoding="UTF-8"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">\#(body)</Relationships>"#
        }
        let contentTypes = """
        <?xml version="1.0" encoding="UTF-8"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">\
        <Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/>\
        <Default Extension="xml" ContentType="application/xml"/>\
        <Default Extension="png" ContentType="image/png"/>\
        <Override PartName="/xl/workbook.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml"/>\
        <Override PartName="/xl/worksheets/sheet1.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml"/>\
        <Override PartName="/xl/drawings/drawing1.xml" ContentType="application/vnd.openxmlformats-officedocument.drawing+xml"/>\
        </Types>
        """
        let workbookParts: [(String, String)] = [
            ("[Content_Types].xml", contentTypes),
            ("_rels/.rels", rels([("rOffice", "officeDocument", "xl/workbook.xml")])),
            ("xl/workbook.xml", """
            <?xml version="1.0" encoding="UTF-8"?><workbook xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" \
            xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships">\
            <sheets><sheet name="Pics" sheetId="1" r:id="rSheet"/></sheets></workbook>
            """),
            ("xl/_rels/workbook.xml.rels", rels([("rSheet", "worksheet", "worksheets/sheet1.xml")])),
            ("xl/worksheets/sheet1.xml", """
            <?xml version="1.0" encoding="UTF-8"?><worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" \
            xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships">\
            <sheetFormatPr defaultRowHeight="20" defaultColWidth="10"/>\
            <sheetData><row r="1" ht="20"><c r="A1" t="inlineStr"><is><t>Pic</t></is></c></row></sheetData>\
            <drawing r:id="rDrawing"/></worksheet>
            """),
            ("xl/worksheets/_rels/sheet1.xml.rels", rels([("rDrawing", "drawing", "../drawings/drawing1.xml")])),
            ("xl/drawings/_rels/drawing1.xml.rels", rels([("rImage", "image", "../media/image1.png")])),
            ("xl/drawings/drawing1.xml", """
            <?xml version="1.0" encoding="UTF-8"?>\
            <xdr:wsDr xmlns:xdr="http://schemas.openxmlformats.org/drawingml/2006/spreadsheetDrawing" \
            xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" \
            xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships">\
            <xdr:twoCellAnchor editAs="oneCell">\
            <xdr:from><xdr:col>0</xdr:col><xdr:colOff>127000</xdr:colOff><xdr:row>1</xdr:row><xdr:rowOff>127000</xdr:rowOff></xdr:from>\
            <xdr:to><xdr:col>2</xdr:col><xdr:colOff>0</xdr:colOff><xdr:row>6</xdr:row><xdr:rowOff>0</xdr:rowOff></xdr:to>\
            <xdr:pic><xdr:nvPicPr><xdr:cNvPr id="2" name="Picture 1"/><xdr:cNvPicPr/></xdr:nvPicPr>\
            <xdr:blipFill><a:blip r:embed="rImage"/><a:stretch><a:fillRect/></a:stretch></xdr:blipFill>\
            <xdr:spPr><a:xfrm/><a:prstGeom prst="rect"><a:avLst/></a:prstGeom></xdr:spPr></xdr:pic>\
            <xdr:clientData/></xdr:twoCellAnchor>\
            <xdr:twoCellAnchor editAs="oneCell">\
            <xdr:from><xdr:col>2</xdr:col><xdr:colOff>254000</xdr:colOff><xdr:row>1</xdr:row><xdr:rowOff>127000</xdr:rowOff></xdr:from>\
            <xdr:to><xdr:col>4</xdr:col><xdr:colOff>0</xdr:colOff><xdr:row>6</xdr:row><xdr:rowOff>0</xdr:rowOff></xdr:to>\
            <xdr:sp><xdr:nvSpPr><xdr:cNvPr id="3" name="Shape 1"/><xdr:cNvSpPr/></xdr:nvSpPr>\
            <xdr:spPr><a:xfrm/><a:prstGeom prst="rect"><a:avLst/></a:prstGeom>\
            <a:solidFill><a:srgbClr val="FF0000"/></a:solidFill>\
            <a:ln><a:noFill/></a:ln></xdr:spPr></xdr:sp>\
            <xdr:clientData/></xdr:twoCellAnchor>\
            </xdr:wsDr>
            """),
        ]
        var entries: [(String, Data)] = workbookParts.map { ($0.0, Data($0.1.utf8)) }
        entries.append(("xl/media/image1.png", Data(Self.onePixelBluePNG)))
        return makeStoredZip(entries: entries)
    }

    /// 把固件写入临时目录并返回文件 URL；调用方负责清理目录。
    private func writeTemporaryFile(named name: String, data: Data) throws -> (dir: URL, url: URL) {
        let dir = FileManager.default.temporaryDirectory
            .appendingPathComponent("jz-office-xlsx-images-\(UUID().uuidString)", isDirectory: true)
        try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        let url = dir.appendingPathComponent(name)
        try data.write(to: url)
        return (dir, url)
    }

    /// 在指定视口参数下离屏绘制工作表，返回自上而下的 RGBA 像素缓冲与采样器。
    ///
    /// 先 `translate + flip` 把上下文调整为 `NSView.isFlipped` 的左上原点坐标系，
    /// 与 `OfficeDocumentNSView.draw` 的实际调用完全一致，因此采样点即屏幕上看到的点。
    private func renderSheet(
        _ sheet: OfficeSheet,
        viewport: CGRect,
        images: [String: CGImage]
    ) -> (sampler: (Int, Int) -> (r: UInt8, g: UInt8, b: UInt8, a: UInt8), image: CGImage) {
        let width = Int(viewport.width)
        let height = Int(viewport.height)
        let colorSpace = CGColorSpace(name: CGColorSpace.sRGB) ?? CGColorSpaceCreateDeviceRGB()
        var bytes = [UInt8](repeating: 255, count: width * height * 4)
        let ctx = CGContext(
            data: &bytes,
            width: width,
            height: height,
            bitsPerComponent: 8,
            bytesPerRow: width * 4,
            space: colorSpace,
            bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue
        )!
        ctx.translateBy(x: 0, y: CGFloat(height))
        ctx.scaleBy(x: 1, y: -1)
        ctx.translateBy(x: -viewport.minX, y: -viewport.minY)
        CGElementDrawer.drawSpreadsheet(
            in: ctx,
            sheet: sheet,
            styles: [],
            viewport: viewport,
            images: images
        )
        let image = ctx.makeImage()!
        let sampler: (Int, Int) -> (r: UInt8, g: UInt8, b: UInt8, a: UInt8) = { x, y in
            let idx = (y * width + x) * 4
            return (bytes[idx], bytes[idx + 1], bytes[idx + 2], bytes[idx + 3])
        }
        return (sampler, image)
    }

    @Test("XLSX 工作表内嵌图片与形状落画：视口可见部件收集、图片解码绘制与形状色块")
    func testXlsxDrawsEmbeddedImagesAndShapes() throws {
        let fixture = makeXlsxWithPictureAndShape()
        let (dir, url) = try writeTemporaryFile(named: "pics.xlsx", data: fixture)
        defer { try? FileManager.default.removeItem(at: dir) }

        // 1. 包内图片部件可被 NativeCore 读出并由 ImageStore 真实解码
        let imageData = try NativeCore.readPart(at: url.path, part: "xl/media/image1.png")
        #expect(imageData.count == Self.onePixelBluePNG.count, "读取到的图片字节应与写入的 PNG 一致")
        let store = ImageStore(filePath: url.path)
        defer { store.release() }
        let decoded = store.loadSync(part: "xl/media/image1.png", maxPixelBudget: 4_000_000)
        #expect(decoded != nil, "ImageStore 必须能解码包内 PNG")
        #expect(decoded?.width == 1 && decoded?.height == 1)

        // 2. 端到端：核心解析结果必须真的带出 <xdr:pic> 的 IMAGE 与 <xdr:sp> 的形状元素，
        //    并且视口收集器能命中这些真实解析出的图片部件。
        let parsed = try NativeCore.parseDocument(at: url.path)
        let parsedSheet = try #require(parsed.sheets.first, "解析结果必须含至少一个工作表")
        let parsedImages = parsedSheet.elements.filter { $0.type == .image }
        let parsedShapes = parsedSheet.elements.filter { $0.type == .rect || $0.type == .path }
        #expect(
            parsedImages.contains { $0.image == "xl/media/image1.png" },
            "核心应输出指向 xl/media/image1.png 的 IMAGE 元素，实际 \(parsedImages.map { $0.image ?? "nil" })"
        )
        #expect(!parsedShapes.isEmpty, "核心应输出 <xdr:sp> 对应的 RECT/PATH 形状元素")
        let parsedViewport = CGRect(x: 0, y: 0, width: 400, height: 300)
        let parsedParts = CGElementDrawer.visibleSheetImageParts(sheet: parsedSheet, viewport: parsedViewport)
        #expect(
            parsedParts.contains("xl/media/image1.png"),
            "视口收集器必须命中真实解析出的图片部件，实际 \(parsedParts)"
        )

        // 2b. 真实解析出的工作表直接离屏绘制：图片像素与形状色块都必须落画。
        //     这一步完全走「core 解析 → 视口收集 → ImageStore 解码 → drawSpreadsheet」的端到端链路。
        let parsedRender = renderSheet(parsedSheet, viewport: parsedViewport, images: store.snapshot)
        // 解析出的图片元素外框 (10,30)-(105,120) 加 42/22 标头 gutter → 屏幕 (52,52)-(147,142)
        let parsedImagePixel = parsedRender.sampler(100, 95)
        #expect(
            parsedImagePixel.b > 200 && parsedImagePixel.r < 60 && parsedImagePixel.g < 60,
            "真实解析出的工作表内嵌图片应绘制为蓝色 PNG，实际 \(parsedImagePixel)"
        )
        // 解析出的形状外框 (125,30)-(210,120) → 屏幕 (167,52)-(252,142)
        let parsedShapePixel = parsedRender.sampler(210, 95)
        #expect(
            parsedShapePixel.r > 200 && parsedShapePixel.g < 60 && parsedShapePixel.b < 60,
            "真实解析出的工作表形状应绘制为红色，实际 \(parsedShapePixel)"
        )

        // 3. 绘制链路：用真实图片部件 + 真实解码结果跑完整绘制，像素上同时验证图片与形状。
        //    模型元素直接构造（等价于 core 对 <xdr:pic>/<xdr:sp> 的输出），
        //    因此本段不依赖 core 的图片编译是否已落地，仍然真实验证「解码 → 视口收集 → 绘制」。
        let picture = OfficeElement(
            type: .image,
            x: 10,
            y: 10,
            width: 60,
            height: 40,
            transform: [1, 0, 0, 1, 10, 10],
            padding: 0,
            fill: 0,
            stroke: 0,
            image: "xl/media/image1.png"
        )
        let shape = OfficeElement(
            type: .rect,
            x: 90,
            y: 10,
            width: 60,
            height: 40,
            transform: [1, 0, 0, 1, 90, 10],
            padding: 0,
            fill: 0xffff0000,
            stroke: 0
        )
        // 完全落在视口外的图片：不得进入可见集合，也不得画进画布
        let offscreen = OfficeElement(
            type: .image,
            x: 900,
            y: 700,
            width: 40,
            height: 40,
            transform: [1, 0, 0, 1, 900, 700],
            padding: 0,
            fill: 0,
            stroke: 0,
            image: "xl/media/image2.png"
        )
        let sheet = OfficeSheet(
            name: "Pics",
            rowHeights: Array(repeating: 20, count: 12),
            columnWidths: Array(repeating: 64, count: 12),
            cells: [],
            merges: [],
            elements: [picture, shape, offscreen]
        )
        let viewport = CGRect(x: 0, y: 0, width: 400, height: 300)

        // 可见部件收集：命中图内图片、跳过视口外图片、保序去重
        let parts = CGElementDrawer.visibleSheetImageParts(sheet: sheet, viewport: viewport)
        #expect(parts == ["xl/media/image1.png"], "可见部件应只有视口内的 image1，实际 \(parts)")

        // 无图片字典时图片退化成灰色占位块，形状仍然绘制：作为「图片确实来自缓存」的对照基线
        let placeholder = renderSheet(sheet, viewport: viewport, images: [:])
        let placeholderPixel = placeholder.sampler(82, 52)
        #expect(
            abs(Int(placeholderPixel.r) - 0xe8) <= 8
                && abs(Int(placeholderPixel.g) - 0xea) <= 8
                && abs(Int(placeholderPixel.b) - 0xed) <= 8,
            "缺少图片缓存时应绘制灰色占位块，实际 \(placeholderPixel)"
        )

        // 传入真实解码结果：图片区域必须变成 PNG 的蓝色，形状区域必须是红色
        let rendered = renderSheet(sheet, viewport: viewport, images: store.snapshot)
        // 图片：数据区原点为 (42,22)，元素 (10,10)-(70,50) → 屏幕 (52,32)-(112,72)，取中心 (82,52)
        let imagePixel = rendered.sampler(82, 52)
        #expect(
            imagePixel.b > 200 && imagePixel.r < 60 && imagePixel.g < 60,
            "内嵌图片应绘制为 PNG 的纯蓝像素，实际 \(imagePixel)"
        )
        // 形状：(90,10)-(150,50) → 屏幕 (132,32)-(192,72)，取中心 (162,52)
        let shapePixel = rendered.sampler(162, 52)
        #expect(
            shapePixel.r > 200 && shapePixel.g < 60 && shapePixel.b < 60,
            "红色填充的 <xdr:sp> 形状应绘制为红色，实际 \(shapePixel)"
        )
        // 视口外图片不得绘制：其元素外框换算到屏幕后完全在画布外，画布右侧保持白底
        let outsidePixel = rendered.sampler(390, 290)
        #expect(
            outsidePixel.r > 240 && outsidePixel.g > 240 && outsidePixel.b > 240,
            "视口外图片不得落画，实际 \(outsidePixel)"
        )

        // 4. 缩略图链路：走真实 XLSX 文件 + parseFirstViewport + 同步图片解码
        let thumb = try OfficeThumbnail.renderCGImage(from: url, maxSize: CGSize(width: 480, height: 480))
        #expect(thumb.width > 0 && thumb.height > 0)
        #expect(hasNonWhitePixel(thumb), "含图片/形状的工作表缩略图不应为空白")
        // 缩略图覆盖首屏可见区，因此必须真的出现蓝色图片像素与红色形状像素
        let thumbSampler = try #require(pixelSampler(for: thumb))
        #expect(
            thumbSampler.contains { $0.b > 180 && $0.r < 90 && $0.g < 90 },
            "缩略图应包含内嵌 PNG 的蓝色像素"
        )
        #expect(
            thumbSampler.contains { $0.r > 180 && $0.g < 90 && $0.b < 90 },
            "缩略图应包含红色填充形状的像素"
        )
    }

    @Test("XLSX 预览视图按视口收集图片部件：可见区与预取区分别命中且切表后刷新")
    @MainActor
    func testXlsxCollectWantedImageParts() throws {
        // 两张表各含不同图片部件：切表必须换掉所需图片集合（否则会一直显示上一表的图片/占位块）
        let doc = try JSONDecoder().decode(OfficeDocument.self, from: """
        {
            "schemaVersion": 9,
            "kind": "XLSX",
            "sheets": [
                {
                    "name": "S1",
                    "rowHeights": [20, 20, 20, 20, 20, 20, 20, 20, 20, 20],
                    "columnWidths": [64, 64, 64, 64, 64, 64, 64, 64, 64, 64],
                    "cells": [],
                    "merges": [],
                    "elements": [
                        {"type": "IMAGE", "x": 10, "y": 10, "width": 40, "height": 40,
                         "transform": [1, 0, 0, 1, 10, 10], "image": "xl/media/near.png"},
                        {"type": "IMAGE", "x": 10, "y": 900, "width": 40, "height": 40,
                         "transform": [1, 0, 0, 1, 10, 900], "image": "xl/media/far.png"}
                    ]
                },
                {
                    "name": "S2",
                    "rowHeights": [20, 20, 20, 20, 20, 20, 20, 20, 20, 20],
                    "columnWidths": [64, 64, 64, 64, 64, 64, 64, 64, 64, 64],
                    "cells": [],
                    "merges": [],
                    "elements": [
                        {"type": "IMAGE", "x": 10, "y": 10, "width": 40, "height": 40,
                         "transform": [1, 0, 0, 1, 10, 10], "image": "xl/media/second.png"}
                    ]
                }
            ],
            "cellStyles": []
        }
        """.data(using: .utf8)!)

        let view = OfficeDocumentNSView()
        view.frame = CGRect(x: 0, y: 0, width: 400, height: 300)
        view.setDocument(doc)
        view.currentPage = 1
        view.zoom = 1

        // 视口 400x300，预取区在垂直方向上下各扩 75%（即 y 约 -300...600）
        let docVisible = CGRect(x: 0, y: 0, width: 400, height: 300)
        let prefetch = docVisible.insetBy(dx: 0, dy: -docVisible.height * 0.75)
        let first = view.collectWantedParts(in: docVisible, prefetch: prefetch)
        #expect(first.visible.contains("xl/media/near.png"), "视口内图片必须进入可见集合，实际 \(first.visible)")
        #expect(
            !first.visible.contains("xl/media/far.png"),
            "视口外图片不得进入可见集合，实际 \(first.visible)"
        )
        // far.png 在 y=900，落在预取区（到 y≈600）之外，因此两个集合都不应包含它
        #expect(
            !first.prefetch.contains("xl/media/far.png"),
            "预取区外图片不得进入预取集合，实际 \(first.prefetch)"
        )

        // 切表：集合必须换成第二张表的图片，且不再要求第一张表的部件
        view.currentPage = 2
        let second = view.collectWantedParts(in: docVisible, prefetch: prefetch)
        #expect(
            second.visible.contains("xl/media/second.png"),
            "切表后应要求新工作表的图片，实际 \(second.visible)"
        )
        #expect(
            !second.visible.contains("xl/media/near.png"),
            "切表后不得继续要求上一张表的图片，实际 \(second.visible)"
        )
    }

    @Test("XLSX exportPDF 输出含工作表图片的有效 PDF")
    func testXlsxExportPDFIncludesSheetImages() throws {
        let fixture = makeXlsxWithPictureAndShape()
        let (dir, url) = try writeTemporaryFile(named: "pics.pdf.xlsx", data: fixture)
        defer { try? FileManager.default.removeItem(at: dir) }

        // 端到端基线：核心必须把 <xdr:pic> 编译成带图片部件的工作表元素，
        // 否则下面的导出断言即使「PASS」也说明不了图片进入了 PDF。
        let parsed = try NativeCore.parseDocument(at: url.path)
        let sheet = try #require(parsed.sheets.first)
        let sheetImageParts = sheet.elements.filter { $0.type == .image }.compactMap(\.image)
        #expect(
            sheetImageParts.contains("xl/media/image1.png"),
            "核心应为工作表输出 xl/media/image1.png，实际 \(sheetImageParts)"
        )

        let pdfURL = dir.appendingPathComponent("out.pdf")
        try OfficeExporter.exportPDF(from: url, to: pdfURL)

        let data = try Data(contentsOf: pdfURL)
        #expect(data.count > 0, "导出 PDF 不应为空")
        #expect(data.prefix(5) == Data("%PDF-".utf8), "导出文件必须是 PDF 头")

        // 能被子系统 PDF 解析器打开，且至少一页
        let document = try #require(
            CGPDFDocument(pdfURL as CFURL),
            "CGPDFDocument 必须能打开导出的 PDF"
        )
        #expect(document.numberOfPages >= 1, "导出的 PDF 至少应有一页")

        // 导出必须真的把工作表画进页面：渲染首页并确认非空白
        let page = try #require(document.page(at: 1))
        let box = page.getBoxRect(.mediaBox)
        #expect(box.width > 0 && box.height > 0)
        let scale: CGFloat = 1
        let width = max(1, Int(box.width * scale))
        let height = max(1, Int(box.height * scale))
        let colorSpace = CGColorSpace(name: CGColorSpace.sRGB) ?? CGColorSpaceCreateDeviceRGB()
        var bytes = [UInt8](repeating: 255, count: width * height * 4)
        let ctx = try #require(CGContext(
            data: &bytes,
            width: width,
            height: height,
            bitsPerComponent: 8,
            bytesPerRow: width * 4,
            space: colorSpace,
            bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue
        ))
        ctx.drawPDFPage(page)
        let drawn = try #require(ctx.makeImage())
        #expect(hasNonWhitePixel(drawn), "导出的 PDF 首页不应是纯白空页")
        // 仅「非空白」不足以证明图片进了 PDF（红色形状也会让页面非空白），
        // 因此额外要求首页出现内嵌 PNG 的蓝色像素：这正是工作表图片预加载与透传的证据。
        let pdfPixels = try #require(pixelSampler(for: drawn))
        #expect(
            pdfPixels.contains { $0.b > 180 && $0.r < 90 && $0.g < 90 },
            "导出的 PDF 首页应包含工作表内嵌 PNG 的蓝色像素"
        )
        #expect(
            pdfPixels.contains { $0.r > 180 && $0.g < 90 && $0.b < 90 },
            "导出的 PDF 首页应包含红色填充形状的像素"
        )
    }
}

/// ZIP 固件构造用的小端追加助手：ZIP 头部字段全部是小端定长整数。
private extension Data {
    mutating func append(littleEndian value: UInt16) {
        append(contentsOf: [UInt8(value & 0xff), UInt8((value >> 8) & 0xff)])
    }

    mutating func append(littleEndian value: UInt32) {
        append(contentsOf: [
            UInt8(value & 0xff),
            UInt8((value >> 8) & 0xff),
            UInt8((value >> 16) & 0xff),
            UInt8((value >> 24) & 0xff),
        ])
    }
}
