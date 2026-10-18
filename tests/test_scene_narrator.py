import unittest

from python.scene_narrator import prompt_for_language, validate_narration


class NarrationLanguageTests(unittest.TestCase):
    def test_hindi_prompt_removes_english_only_instruction(self):
        prompt = prompt_for_language(
            "You are producing one continuous English prose narration.\n"
            "LANGUAGE\n- Output STRICTLY in English. Translate source text.\n",
            "hindi",
        )
        self.assertNotIn("STRICTLY in English", prompt)
        self.assertIn("Devanagari", prompt)

    def test_hindi_rejects_parenthetical_english_duplication(self):
        text = (
            "वह पहली बार उस भयानक दानव को देखकर स्तब्ध रह गया। "
            "(This was the first time he had seen the demon.) "
            "उसके हाथ कांपने लगे और कमरे में गहरा सन्नाटा छा गया।"
        )
        self.assertTrue(validate_narration(text, "hindi"))

    def test_grounded_english_and_hindi_pass(self):
        english = (
            "The black-haired swordsman stepped into the ruined hall and tightened his grip. "
            "Dust drifted through the broken roof while the masked stranger watched in silence. "
            "\"Stay back,\" the swordsman warned, as a shadow moved behind the cracked pillars."
        )
        hindi = (
            "काले बालों वाला तलवारबाज़ टूटे हुए कक्ष में धीरे-धीरे आगे बढ़ा और उसने अपनी पकड़ मजबूत कर ली। "
            "टूटी छत से धूल नीचे गिर रही थी, जबकि नकाबपोश अजनबी चुपचाप उसे देख रहा था। "
            "तलवारबाज़ ने चेतावनी दी, \"पीछे रहो,\" तभी स्तंभों के पीछे एक परछाई हिली।"
        )
        self.assertEqual(validate_narration(english, "english"), [])
        self.assertEqual(validate_narration(hindi, "hindi"), [])


if __name__ == "__main__":
    unittest.main()
