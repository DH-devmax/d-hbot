#!/usr/bin/env python3
"""Build the current production DH BOT Chinese manual with annotated screenshots."""

from pathlib import Path
import sys

from reportlab.lib import colors
from reportlab.lib.pagesizes import A4, landscape
from reportlab.lib.styles import ParagraphStyle
from reportlab.lib.units import mm
from reportlab.pdfbase import pdfmetrics
from reportlab.pdfbase.ttfonts import TTFont
from reportlab.pdfgen import canvas
from reportlab.platypus import Paragraph
from reportlab.lib.utils import ImageReader


ROOT = Path(__file__).resolve().parents[1]
PREVIEW = ROOT / "docs" / "screenshots" / "tauri3"
FONT_NAME = "DHManual"
FONT_CANDIDATES = [
    Path("/System/Library/Fonts/Supplemental/Arial Unicode.ttf"),
    Path("/Library/Fonts/Arial Unicode.ttf"),
]


PAGES = [
    ("总览", "overview.png", "先看旺商聊连接、本地数据库、已启用的 AI 群和最近审计。每日摘要只在本机总览显示，不发送到群里。", [(0.91, 0.86, "连接状态"), (0.57, 0.62, "最近审计")]),
    ("群组与成员", "groups.png", "先选择群，再切换群组和成员。AI 自动化、固定规则、群名片自动化和人工群控彼此独立，按需开启。", [(0.27, 0.82, "群组 / 成员"), (0.42, 0.70, "搜索群组")]),
    ("消息台", "messages.png", "按群名称选择一个或多个群后发送人工文本。最近消息按文本、图片、名片和其他分类，重复消息不会重复执行。", [(0.47, 0.76, "选择群"), (0.57, 0.48, "消息记录")]),
    ("规则", "rules.png", "规则可全局或按群生效。新规则默认观察；建议先只勾选撤回，确认后再开启禁言、移出等高影响动作。", [(0.89, 0.80, "新建规则"), (0.75, 0.80, "导入 / 导出")]),
    ("知识与 AI", "knowledge.png", "先新建或选择知识库，保存文档后勾选使用群。没有绑定群的知识库不会参与 AI 回答。", [(0.88, 0.80, "新建知识库"), (0.64, 0.80, "刷新列表")]),
    ("任务与计划", "tasks.png", "任务保存负责人、截止和提醒。开关群计划按电脑时区执行；每日摘要只写到本机总览。", [(0.29, 0.70, "新建任务"), (0.78, 0.80, "功能标签")]),
    ("审计", "audit.png", "这里查看连接、成员、规则、AI、任务和群管动作。可按群、级别、日期和关键词筛选，也可导出 CSV 或 JSON。", [(0.45, 0.76, "筛选与导出"), (0.57, 0.53, "审计记录")]),
    ("设置", "settings.png", "在这里选择旺商聊 EXE、启动或显示旺商聊，并填写 AI 服务地址、模型和密钥。生产版固定连接 9222。", [(0.34, 0.75, "旺商聊连接"), (0.70, 0.66, "AI 设置")]),
    ("AI 离群测试", "ai-test.png", "只测试当前 AI 设置，不读取群消息，不发送群消息，也不会执行撤回、禁言、移出或创建真实任务。", [(0.25, 0.75, "输入问题"), (0.40, 0.45, "查看回复")]),
    ("调试", "debug.png", "连接异常先点立即检查。这里展示 DevTools、NIM、能力校准、本地数据库和固定登录分区维护状态。", [(0.80, 0.52, "NIM 与能力"), (0.80, 0.42, "固定分区")]),
]


def register_font():
    for candidate in FONT_CANDIDATES:
        if candidate.exists():
            pdfmetrics.registerFont(TTFont(FONT_NAME, str(candidate)))
            return
    raise FileNotFoundError("未找到可嵌入的 Unicode 中文字体")


def draw_arrow(c, x, y, tx, ty, label):
    c.setStrokeColor(colors.HexColor("#D64045"))
    c.setFillColor(colors.HexColor("#D64045"))
    c.setLineWidth(1.5)
    c.line(x, y, tx, ty)
    dx, dy = tx - x, ty - y
    length = max((dx * dx + dy * dy) ** 0.5, 1.0)
    ux, uy = dx / length, dy / length
    size = 6
    left = (tx - ux * size - uy * size / 2, ty - uy * size + ux * size / 2)
    right = (tx - ux * size + uy * size / 2, ty - uy * size - ux * size / 2)
    path = c.beginPath()
    path.moveTo(tx, ty)
    path.lineTo(*left)
    path.lineTo(*right)
    path.close()
    c.drawPath(path, fill=1, stroke=0)
    c.setFont(FONT_NAME, 9)
    c.drawString(x + 4, y + 4, label)


def draw_screenshot_page(c, title, image_path, note, arrows, page_number, total_pages):
    width, height = landscape(A4)
    c.setFillColor(colors.HexColor("#FFFFFF"))
    c.rect(0, 0, width, height, fill=1, stroke=0)
    c.setFillColor(colors.HexColor("#111111"))
    c.setFont(FONT_NAME, 18)
    c.drawString(18 * mm, height - 15 * mm, title)
    c.setFillColor(colors.HexColor("#777777"))
    c.setFont(FONT_NAME, 9)
    c.drawRightString(width - 18 * mm, height - 14.5 * mm, "DH BOT 3.0 使用手册")

    image = ImageReader(str(image_path))
    iw, ih = image.getSize()
    max_w, max_h = width - 36 * mm, height - 68 * mm
    scale = min(max_w / iw, max_h / ih)
    draw_w, draw_h = iw * scale, ih * scale
    x, y = (width - draw_w) / 2, 29 * mm
    c.setStrokeColor(colors.HexColor("#DDDDDD"))
    c.rect(x - 1, y - 1, draw_w + 2, draw_h + 2, fill=0, stroke=1)
    c.drawImage(image, x, y, width=draw_w, height=draw_h, preserveAspectRatio=True, mask="auto")
    for nx, ny, label in arrows:
        tx, ty = x + nx * draw_w, y + ny * draw_h
        draw_arrow(c, min(width - 35 * mm, tx + 70), min(height - 25 * mm, ty + 55), tx, ty, label)

    style = ParagraphStyle("note", fontName=FONT_NAME, fontSize=9.5, leading=13, textColor=colors.HexColor("#333333"))
    paragraph = Paragraph(note, style)
    paragraph.wrapOn(c, width - 36 * mm, 17 * mm)
    paragraph.drawOn(c, 18 * mm, 8 * mm)
    c.setFont(FONT_NAME, 8)
    c.setFillColor(colors.HexColor("#999999"))
    c.drawRightString(width - 18 * mm, 9 * mm, f"{title} · {page_number}/{total_pages}")
    c.showPage()


def main(output: str):
    output_path = Path(output)
    output_path.parent.mkdir(parents=True, exist_ok=True)
    register_font()
    c = canvas.Canvas(str(output_path), pagesize=landscape(A4))
    c.setTitle("DH BOT 3.0 大白话使用手册")
    c.setAuthor("DH BOT")
    c.setSubject("旺商聊 AI 群管理工具图解手册")
    width, height = landscape(A4)
    c.setFillColor(colors.HexColor("#111111"))
    c.setFont(FONT_NAME, 34)
    c.drawString(24 * mm, height - 45 * mm, "DH BOT 3.0")
    c.setFont(FONT_NAME, 20)
    c.drawString(24 * mm, height - 60 * mm, "大白话使用手册")
    c.setFont(FONT_NAME, 12)
    c.setFillColor(colors.HexColor("#666666"))
    c.drawString(24 * mm, height - 78 * mm, "正式版图解: 旺商聊、群管理、AI、知识库与计划")
    c.setFont(FONT_NAME, 11)
    c.drawString(24 * mm, 30 * mm, "Windows：双击 DH-BOT.exe。生产版固定连接本机旺商聊 9222，不含开发测试工具。")
    c.setFont(FONT_NAME, 8)
    c.drawRightString(width - 18 * mm, 9 * mm, f"1/{len(PAGES) + 1}")
    c.showPage()

    total_pages = len(PAGES) + 1
    for page_number, (title, filename, note, arrows) in enumerate(PAGES, start=2):
        image_path = PREVIEW / filename
        if not image_path.exists():
            raise FileNotFoundError(f"缺少 DH BOT 3.0 当前版截图：{image_path}")
        draw_screenshot_page(c, title, image_path, note, arrows, page_number, total_pages)

    c.save()
    print(output_path)


if __name__ == "__main__":
    main(sys.argv[1] if len(sys.argv) > 1 else str(ROOT / "docs" / "DH-Manual-ZH.pdf"))
