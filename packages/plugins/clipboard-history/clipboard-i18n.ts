export type ClipboardLocale = "en" | "zh" | "ja" | "ko" | "de" | "fr" | "ru";

export interface ClipboardCopy {
  title: string;
  back: string;
  refresh: string;
  searchPlaceholder: string;
  loading: string;
  emptyTitle: string;
  emptyDescription: string;
  noMatchesTitle: string;
  noMatchesDescription: string;
  unavailable: string;
  copied: string;
  copyFailed: string;
  pinFailed: string;
  pin: string;
  pinned: string;
  unpin: string;
  copy: string;
  whitespaceOnly: string;
  justNow: string;
  summary: (total: number, pinned: number) => string;
  characters: (count: number) => string;
  minutesAgo: (count: number) => string;
  hoursAgo: (count: number) => string;
  daysAgo: (count: number) => string;
  hint: (embedded: boolean) => string;
}

export function normalizeClipboardLocale(locale: string): ClipboardLocale {
  const language = locale.toLowerCase().replaceAll("_", "-").split("-")[0];
  switch (language) {
    case "zh":
      return "zh";
    case "ja":
      return "ja";
    case "ko":
      return "ko";
    case "de":
      return "de";
    case "fr":
      return "fr";
    case "ru":
      return "ru";
    default:
      return "en";
  }
}

export function clipboardCopyFor(locale: string): ClipboardCopy {
  switch (normalizeClipboardLocale(locale)) {
    case "zh":
      return {
        title: "剪贴板历史",
        back: "返回",
        refresh: "刷新",
        searchPlaceholder: "搜索剪贴板内容…",
        loading: "正在加载剪贴板历史…",
        emptyTitle: "剪贴板还是空的",
        emptyDescription: "复制一些文本后，它们会显示在这里。",
        noMatchesTitle: "没有匹配内容",
        noMatchesDescription: "换一个关键词试试。",
        unavailable: "剪贴板历史暂时不可用。",
        copied: "已复制到剪贴板",
        copyFailed: "复制失败",
        pinFailed: "置顶状态未能保存",
        pin: "置顶",
        pinned: "已置顶",
        unpin: "取消置顶",
        copy: "复制",
        whitespaceOnly: "（仅包含空白字符）",
        justNow: "刚刚",
        summary: (total, pinned) => `${total} 条记录 · ${pinned} 条置顶`,
        characters: (count) => `${count} 个字符`,
        minutesAgo: (count) => `${count} 分钟前`,
        hoursAgo: (count) => `${count} 小时前`,
        daysAgo: (count) => `${count} 天前`,
        hint: (embedded) => `↑↓ 选择 · Enter 复制${embedded ? " · Esc 返回" : ""}`,
      };
    case "ja":
      return {
        title: "クリップボード履歴",
        back: "戻る",
        refresh: "更新",
        searchPlaceholder: "クリップボードを検索…",
        loading: "クリップボード履歴を読み込み中…",
        emptyTitle: "クリップボードは空です",
        emptyDescription: "テキストをコピーすると、ここに表示されます。",
        noMatchesTitle: "一致する項目はありません",
        noMatchesDescription: "別のキーワードを試してください。",
        unavailable: "クリップボード履歴を利用できません。",
        copied: "クリップボードにコピーしました",
        copyFailed: "コピーに失敗しました",
        pinFailed: "ピン留めを保存できませんでした",
        pin: "ピン",
        pinned: "ピン済み",
        unpin: "ピンを解除",
        copy: "コピー",
        whitespaceOnly: "（空白のみ）",
        justNow: "たった今",
        summary: (total, pinned) => `${total} 件 · ピン ${pinned} 件`,
        characters: (count) => `${count} 文字`,
        minutesAgo: (count) => `${count} 分前`,
        hoursAgo: (count) => `${count} 時間前`,
        daysAgo: (count) => `${count} 日前`,
        hint: (embedded) => `↑↓ 選択 · Enter コピー${embedded ? " · Esc 戻る" : ""}`,
      };
    case "ko":
      return {
        title: "클립보드 기록",
        back: "뒤로",
        refresh: "새로 고침",
        searchPlaceholder: "클립보드 검색…",
        loading: "클립보드 기록을 불러오는 중…",
        emptyTitle: "클립보드가 비어 있습니다",
        emptyDescription: "텍스트를 복사하면 여기에 표시됩니다.",
        noMatchesTitle: "일치하는 항목이 없습니다",
        noMatchesDescription: "다른 검색어를 입력해 보세요.",
        unavailable: "클립보드 기록을 사용할 수 없습니다.",
        copied: "클립보드에 복사했습니다",
        copyFailed: "복사하지 못했습니다",
        pinFailed: "고정 상태를 저장하지 못했습니다",
        pin: "고정",
        pinned: "고정됨",
        unpin: "고정 해제",
        copy: "복사",
        whitespaceOnly: "(공백만 있음)",
        justNow: "방금",
        summary: (total, pinned) => `${total}개 · 고정 ${pinned}개`,
        characters: (count) => `${count}자`,
        minutesAgo: (count) => `${count}분 전`,
        hoursAgo: (count) => `${count}시간 전`,
        daysAgo: (count) => `${count}일 전`,
        hint: (embedded) => `↑↓ 선택 · Enter 복사${embedded ? " · Esc 뒤로" : ""}`,
      };
    case "de":
      return {
        title: "Zwischenablageverlauf",
        back: "Zurück",
        refresh: "Aktualisieren",
        searchPlaceholder: "Zwischenablage durchsuchen…",
        loading: "Zwischenablageverlauf wird geladen…",
        emptyTitle: "Die Zwischenablage ist leer",
        emptyDescription: "Kopierter Text erscheint hier.",
        noMatchesTitle: "Keine Treffer",
        noMatchesDescription: "Versuchen Sie einen anderen Suchbegriff.",
        unavailable: "Der Zwischenablageverlauf ist nicht verfügbar.",
        copied: "In die Zwischenablage kopiert",
        copyFailed: "Kopieren fehlgeschlagen",
        pinFailed: "Anheftung konnte nicht gespeichert werden",
        pin: "Anheften",
        pinned: "Angeheftet",
        unpin: "Lösen",
        copy: "Kopieren",
        whitespaceOnly: "(nur Leerraum)",
        justNow: "gerade eben",
        summary: (total, pinned) => `${total} Einträge · ${pinned} angeheftet`,
        characters: (count) => `${count} Zeichen`,
        minutesAgo: (count) => `vor ${count} Min.`,
        hoursAgo: (count) => `vor ${count} Std.`,
        daysAgo: (count) => `vor ${count} Tagen`,
        hint: (embedded) => `↑↓ Auswählen · Enter Kopieren${embedded ? " · Esc Zurück" : ""}`,
      };
    case "fr":
      return {
        title: "Historique du presse-papiers",
        back: "Retour",
        refresh: "Actualiser",
        searchPlaceholder: "Rechercher dans le presse-papiers…",
        loading: "Chargement de l’historique…",
        emptyTitle: "Le presse-papiers est vide",
        emptyDescription: "Le texte copié apparaîtra ici.",
        noMatchesTitle: "Aucun résultat",
        noMatchesDescription: "Essayez un autre mot-clé.",
        unavailable: "L’historique du presse-papiers est indisponible.",
        copied: "Copié dans le presse-papiers",
        copyFailed: "Échec de la copie",
        pinFailed: "Impossible d’enregistrer l’épinglage",
        pin: "Épingler",
        pinned: "Épinglé",
        unpin: "Détacher",
        copy: "Copier",
        whitespaceOnly: "(espaces uniquement)",
        justNow: "à l’instant",
        summary: (total, pinned) => `${total} éléments · ${pinned} épinglés`,
        characters: (count) => `${count} caractères`,
        minutesAgo: (count) => `il y a ${count} min`,
        hoursAgo: (count) => `il y a ${count} h`,
        daysAgo: (count) => `il y a ${count} j`,
        hint: (embedded) => `↑↓ Choisir · Entrée Copier${embedded ? " · Échap Retour" : ""}`,
      };
    case "ru":
      return {
        title: "История буфера обмена",
        back: "Назад",
        refresh: "Обновить",
        searchPlaceholder: "Поиск в буфере обмена…",
        loading: "Загрузка истории буфера обмена…",
        emptyTitle: "Буфер обмена пуст",
        emptyDescription: "Скопированный текст появится здесь.",
        noMatchesTitle: "Ничего не найдено",
        noMatchesDescription: "Попробуйте другой запрос.",
        unavailable: "История буфера обмена недоступна.",
        copied: "Скопировано в буфер обмена",
        copyFailed: "Не удалось скопировать",
        pinFailed: "Не удалось сохранить закрепление",
        pin: "Закрепить",
        pinned: "Закреплено",
        unpin: "Открепить",
        copy: "Копировать",
        whitespaceOnly: "(только пробелы)",
        justNow: "только что",
        summary: (total, pinned) => `${total} записей · ${pinned} закреплено`,
        characters: (count) => `${count} симв.`,
        minutesAgo: (count) => `${count} мин назад`,
        hoursAgo: (count) => `${count} ч назад`,
        daysAgo: (count) => `${count} дн назад`,
        hint: (embedded) => `↑↓ Выбор · Enter Копировать${embedded ? " · Esc Назад" : ""}`,
      };
    default:
      return {
        title: "Clipboard History",
        back: "Back",
        refresh: "Refresh",
        searchPlaceholder: "Search clipboard history…",
        loading: "Loading clipboard history…",
        emptyTitle: "Clipboard history is empty",
        emptyDescription: "Copy some text and it will appear here.",
        noMatchesTitle: "No matching items",
        noMatchesDescription: "Try a different search term.",
        unavailable: "Clipboard history is unavailable.",
        copied: "Copied to clipboard",
        copyFailed: "Could not copy to clipboard",
        pinFailed: "Pinned items could not be saved",
        pin: "Pin",
        pinned: "Pinned",
        unpin: "Unpin",
        copy: "Copy",
        whitespaceOnly: "(whitespace only)",
        justNow: "just now",
        summary: (total, pinned) => `${total} items · ${pinned} pinned`,
        characters: (count) => `${count} characters`,
        minutesAgo: (count) => `${count}m ago`,
        hoursAgo: (count) => `${count}h ago`,
        daysAgo: (count) => `${count}d ago`,
        hint: (embedded) => `↑↓ Select · Enter Copy${embedded ? " · Esc Back" : ""}`,
      };
  }
}
