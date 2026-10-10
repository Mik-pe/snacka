// SPDX-License-Identifier: MIT
use crate::entropy::{self, Model};

#[cfg(test)]
const DIMENSIONS: [usize; 7] = [20, 40, 80, 160, 320, 400, 800];
const MAX_PULSES: usize = 387;
// ceil(log2(C(N+K-1,K))) plus at most one sign bit per pulse.
const PULSE_COST: [[u16; 388]; 7] = [
    [
        0, 6, 10, 14, 18, 21, 24, 27, 30, 32, 35, 37, 40, 42, 44, 46, 48, 51, 53, 55, 57, 58, 60,
        62, 64, 66, 68, 69, 71, 73, 75, 76, 78, 80, 81, 83, 84, 86, 88, 89, 91, 92, 94, 95, 97, 98,
        100, 101, 103, 104, 106, 107, 109, 110, 112, 113, 114, 116, 117, 119, 120, 122, 123, 124,
        126, 127, 128, 130, 131, 132, 134, 135, 136, 138, 139, 140, 142, 143, 144, 146, 147, 148,
        150, 151, 152, 154, 155, 156, 157, 159, 160, 161, 162, 164, 165, 166, 168, 169, 170, 171,
        173, 174, 175, 176, 178, 179, 180, 181, 182, 184, 185, 186, 187, 189, 190, 191, 192, 194,
        195, 196, 197, 198, 200, 201, 202, 203, 204, 206, 207, 208, 209, 210, 212, 213, 214, 215,
        216, 218, 219, 220, 221, 222, 223, 225, 226, 227, 228, 229, 230, 232, 233, 234, 235, 236,
        238, 239, 240, 241, 242, 243, 245, 246, 247, 248, 249, 250, 251, 253, 254, 255, 256, 257,
        258, 260, 261, 262, 263, 264, 265, 266, 268, 269, 270, 271, 272, 273, 274, 276, 277, 278,
        279, 280, 281, 282, 284, 285, 286, 287, 288, 289, 290, 291, 293, 294, 295, 296, 297, 298,
        299, 300, 302, 303, 304, 305, 306, 307, 308, 309, 311, 312, 313, 314, 315, 316, 317, 318,
        320, 321, 322, 323, 324, 325, 326, 327, 328, 330, 331, 332, 333, 334, 335, 336, 337, 338,
        340, 341, 342, 343, 344, 345, 346, 347, 348, 349, 351, 352, 353, 354, 355, 356, 357, 358,
        359, 361, 362, 363, 364, 365, 366, 367, 368, 369, 370, 371, 373, 374, 375, 376, 377, 378,
        379, 380, 381, 382, 384, 385, 386, 387, 388, 389, 390, 391, 392, 393, 394, 396, 397, 398,
        399, 400, 401, 402, 403, 404, 405, 406, 408, 409, 410, 411, 412, 413, 414, 415, 416, 417,
        418, 419, 421, 422, 423, 424, 425, 426, 427, 428, 429, 430, 431, 432, 434, 435, 436, 437,
        438, 439, 440, 441, 442, 443, 444, 445, 446, 448, 449, 450, 451, 452, 453, 454, 455, 456,
        457, 458, 459, 460, 462, 463, 464, 465, 466, 467, 468, 469, 470, 471, 472, 473, 474, 475,
        477, 478, 479, 480, 481, 482, 483, 484, 485, 486, 487, 488, 489, 490, 492, 493, 494, 495,
    ],
    [
        0, 7, 12, 17, 21, 26, 29, 33, 37, 40, 43, 47, 50, 53, 56, 58, 61, 64, 67, 69, 72, 74, 77,
        79, 82, 84, 86, 89, 91, 93, 95, 97, 100, 102, 104, 106, 108, 110, 112, 114, 116, 118, 120,
        122, 124, 126, 128, 129, 131, 133, 135, 137, 139, 140, 142, 144, 146, 147, 149, 151, 153,
        154, 156, 158, 159, 161, 163, 164, 166, 168, 169, 171, 173, 174, 176, 177, 179, 181, 182,
        184, 185, 187, 189, 190, 192, 193, 195, 196, 198, 199, 201, 202, 204, 205, 207, 208, 210,
        211, 213, 214, 216, 217, 219, 220, 222, 223, 225, 226, 227, 229, 230, 232, 233, 235, 236,
        237, 239, 240, 242, 243, 245, 246, 247, 249, 250, 252, 253, 254, 256, 257, 258, 260, 261,
        263, 264, 265, 267, 268, 269, 271, 272, 273, 275, 276, 277, 279, 280, 281, 283, 284, 286,
        287, 288, 289, 291, 292, 293, 295, 296, 297, 299, 300, 301, 303, 304, 305, 307, 308, 309,
        310, 312, 313, 314, 316, 317, 318, 320, 321, 322, 323, 325, 326, 327, 329, 330, 331, 332,
        334, 335, 336, 337, 339, 340, 341, 343, 344, 345, 346, 348, 349, 350, 351, 353, 354, 355,
        356, 358, 359, 360, 361, 363, 364, 365, 366, 368, 369, 370, 371, 372, 374, 375, 376, 377,
        379, 380, 381, 382, 384, 385, 386, 387, 388, 390, 391, 392, 393, 395, 396, 397, 398, 399,
        401, 402, 403, 404, 406, 407, 408, 409, 410, 412, 413, 414, 415, 416, 418, 419, 420, 421,
        422, 424, 425, 426, 427, 428, 430, 431, 432, 433, 434, 436, 437, 438, 439, 440, 442, 443,
        444, 445, 446, 448, 449, 450, 451, 452, 453, 455, 456, 457, 458, 459, 461, 462, 463, 464,
        465, 466, 468, 469, 470, 471, 472, 474, 475, 476, 477, 478, 479, 481, 482, 483, 484, 485,
        486, 488, 489, 490, 491, 492, 493, 495, 496, 497, 498, 499, 500, 502, 503, 504, 505, 506,
        507, 509, 510, 511, 512, 513, 514, 516, 517, 518, 519, 520, 521, 522, 524, 525, 526, 527,
        528, 529, 531, 532, 533, 534, 535, 536, 537, 539, 540, 541, 542, 543, 544, 545, 547, 548,
        549, 550, 551, 552, 553, 555, 556, 557, 558, 559, 560, 561, 563, 564, 565, 566, 567, 568,
        569, 571, 572,
    ],
    [
        0, 8, 14, 20, 25, 30, 35, 40, 44, 49, 53, 57, 61, 64, 68, 72, 75, 79, 82, 86, 89, 92, 95,
        99, 102, 105, 108, 111, 114, 117, 119, 122, 125, 128, 131, 133, 136, 139, 141, 144, 146,
        149, 151, 154, 156, 159, 161, 164, 166, 169, 171, 173, 176, 178, 180, 183, 185, 187, 189,
        192, 194, 196, 198, 200, 202, 205, 207, 209, 211, 213, 215, 217, 219, 221, 223, 225, 227,
        230, 232, 234, 236, 237, 239, 241, 243, 245, 247, 249, 251, 253, 255, 257, 259, 261, 263,
        264, 266, 268, 270, 272, 274, 275, 277, 279, 281, 283, 285, 286, 288, 290, 292, 294, 295,
        297, 299, 301, 302, 304, 306, 308, 309, 311, 313, 314, 316, 318, 320, 321, 323, 325, 326,
        328, 330, 331, 333, 335, 336, 338, 340, 341, 343, 345, 346, 348, 349, 351, 353, 354, 356,
        358, 359, 361, 362, 364, 366, 367, 369, 370, 372, 374, 375, 377, 378, 380, 381, 383, 385,
        386, 388, 389, 391, 392, 394, 395, 397, 398, 400, 402, 403, 405, 406, 408, 409, 411, 412,
        414, 415, 417, 418, 420, 421, 423, 424, 426, 427, 429, 430, 432, 433, 435, 436, 438, 439,
        441, 442, 443, 445, 446, 448, 449, 451, 452, 454, 455, 457, 458, 460, 461, 462, 464, 465,
        467, 468, 470, 471, 473, 474, 475, 477, 478, 480, 481, 483, 484, 485, 487, 488, 490, 491,
        492, 494, 495, 497, 498, 499, 501, 502, 504, 505, 506, 508, 509, 511, 512, 513, 515, 516,
        518, 519, 520, 522, 523, 525, 526, 527, 529, 530, 531, 533, 534, 535, 537, 538, 540, 541,
        542, 544, 545, 546, 548, 549, 550, 552, 553, 555, 556, 557, 559, 560, 561, 563, 564, 565,
        567, 568, 569, 571, 572, 573, 575, 576, 577, 579, 580, 581, 583, 584, 585, 587, 588, 589,
        591, 592, 593, 595, 596, 597, 599, 600, 601, 603, 604, 605, 607, 608, 609, 611, 612, 613,
        614, 616, 617, 618, 620, 621, 622, 624, 625, 626, 628, 629, 630, 631, 633, 634, 635, 637,
        638, 639, 640, 642, 643, 644, 646, 647, 648, 650, 651, 652, 653, 655, 656, 657, 659, 660,
        661, 662, 664, 665, 666, 667, 669, 670, 671, 673, 674, 675, 676, 678, 679, 680, 682, 683,
        684, 685, 687, 688, 689,
    ],
    [
        0, 9, 16, 23, 29, 35, 41, 47, 52, 57, 62, 67, 72, 77, 81, 86, 90, 95, 99, 103, 108, 112,
        116, 120, 124, 127, 131, 135, 139, 142, 146, 150, 153, 157, 160, 164, 167, 171, 174, 177,
        181, 184, 187, 191, 194, 197, 200, 203, 206, 209, 212, 215, 219, 222, 224, 227, 230, 233,
        236, 239, 242, 245, 248, 250, 253, 256, 259, 262, 264, 267, 270, 272, 275, 278, 280, 283,
        286, 288, 291, 294, 296, 299, 301, 304, 306, 309, 311, 314, 316, 319, 321, 324, 326, 329,
        331, 333, 336, 338, 341, 343, 345, 348, 350, 352, 355, 357, 359, 362, 364, 366, 369, 371,
        373, 376, 378, 380, 382, 385, 387, 389, 391, 393, 396, 398, 400, 402, 404, 407, 409, 411,
        413, 415, 417, 419, 422, 424, 426, 428, 430, 432, 434, 436, 438, 440, 443, 445, 447, 449,
        451, 453, 455, 457, 459, 461, 463, 465, 467, 469, 471, 473, 475, 477, 479, 481, 483, 485,
        487, 489, 491, 493, 495, 497, 499, 501, 502, 504, 506, 508, 510, 512, 514, 516, 518, 520,
        522, 524, 525, 527, 529, 531, 533, 535, 537, 539, 540, 542, 544, 546, 548, 550, 552, 553,
        555, 557, 559, 561, 563, 564, 566, 568, 570, 572, 573, 575, 577, 579, 581, 582, 584, 586,
        588, 590, 591, 593, 595, 597, 598, 600, 602, 604, 605, 607, 609, 611, 613, 614, 616, 618,
        619, 621, 623, 625, 626, 628, 630, 632, 633, 635, 637, 638, 640, 642, 644, 645, 647, 649,
        650, 652, 654, 655, 657, 659, 660, 662, 664, 666, 667, 669, 671, 672, 674, 676, 677, 679,
        681, 682, 684, 686, 687, 689, 690, 692, 694, 695, 697, 699, 700, 702, 704, 705, 707, 708,
        710, 712, 713, 715, 717, 718, 720, 721, 723, 725, 726, 728, 730, 731, 733, 734, 736, 738,
        739, 741, 742, 744, 745, 747, 749, 750, 752, 753, 755, 757, 758, 760, 761, 763, 764, 766,
        768, 769, 771, 772, 774, 775, 777, 779, 780, 782, 783, 785, 786, 788, 789, 791, 793, 794,
        796, 797, 799, 800, 802, 803, 805, 806, 808, 809, 811, 813, 814, 816, 817, 819, 820, 822,
        823, 825, 826, 828, 829, 831, 832, 834, 835, 837, 838, 840, 841, 843, 844, 846, 847, 849,
        850, 852, 853, 855, 856, 858,
    ],
    [
        0, 10, 18, 26, 33, 40, 47, 54, 60, 66, 72, 78, 84, 89, 95, 101, 106, 111, 116, 122, 127,
        132, 137, 142, 146, 151, 156, 161, 165, 170, 174, 179, 183, 188, 192, 196, 201, 205, 209,
        213, 218, 222, 226, 230, 234, 238, 242, 246, 250, 254, 258, 262, 265, 269, 273, 277, 280,
        284, 288, 292, 295, 299, 302, 306, 310, 313, 317, 320, 324, 327, 331, 334, 338, 341, 344,
        348, 351, 355, 358, 361, 365, 368, 371, 374, 378, 381, 384, 387, 391, 394, 397, 400, 403,
        407, 410, 413, 416, 419, 422, 425, 428, 431, 434, 437, 440, 443, 446, 449, 452, 455, 458,
        461, 464, 467, 470, 473, 476, 479, 482, 485, 487, 490, 493, 496, 499, 502, 504, 507, 510,
        513, 516, 518, 521, 524, 527, 530, 532, 535, 538, 540, 543, 546, 549, 551, 554, 557, 559,
        562, 565, 567, 570, 573, 575, 578, 580, 583, 586, 588, 591, 593, 596, 599, 601, 604, 606,
        609, 611, 614, 616, 619, 622, 624, 627, 629, 632, 634, 637, 639, 641, 644, 646, 649, 651,
        654, 656, 659, 661, 664, 666, 668, 671, 673, 676, 678, 681, 683, 685, 688, 690, 692, 695,
        697, 700, 702, 704, 707, 709, 711, 714, 716, 718, 721, 723, 725, 728, 730, 732, 735, 737,
        739, 741, 744, 746, 748, 751, 753, 755, 757, 760, 762, 764, 766, 769, 771, 773, 775, 778,
        780, 782, 784, 787, 789, 791, 793, 795, 798, 800, 802, 804, 806, 809, 811, 813, 815, 817,
        819, 822, 824, 826, 828, 830, 832, 835, 837, 839, 841, 843, 845, 847, 850, 852, 854, 856,
        858, 860, 862, 864, 866, 869, 871, 873, 875, 877, 879, 881, 883, 885, 887, 889, 891, 894,
        896, 898, 900, 902, 904, 906, 908, 910, 912, 914, 916, 918, 920, 922, 924, 926, 928, 930,
        932, 934, 936, 938, 940, 942, 945, 947, 949, 951, 953, 955, 957, 959, 960, 962, 964, 966,
        968, 970, 972, 974, 976, 978, 980, 982, 984, 986, 988, 990, 992, 994, 996, 998, 1000, 1002,
        1004, 1006, 1008, 1010, 1012, 1013, 1015, 1017, 1019, 1021, 1023, 1025, 1027, 1029, 1031,
        1033, 1035, 1037, 1038, 1040, 1042, 1044, 1046, 1048, 1050, 1052, 1054, 1056, 1057, 1059,
        1061, 1063, 1065, 1067, 1069, 1071, 1072, 1074, 1076, 1078, 1080, 1082, 1084,
    ],
    [
        0, 10, 19, 27, 35, 42, 49, 56, 62, 69, 75, 82, 88, 94, 99, 105, 111, 117, 122, 128, 133,
        138, 144, 149, 154, 159, 164, 169, 174, 179, 184, 188, 193, 198, 203, 207, 212, 216, 221,
        225, 230, 234, 239, 243, 247, 252, 256, 260, 264, 269, 273, 277, 281, 285, 289, 293, 297,
        301, 305, 309, 313, 317, 321, 325, 329, 332, 336, 340, 344, 348, 351, 355, 359, 362, 366,
        370, 373, 377, 381, 384, 388, 391, 395, 399, 402, 406, 409, 413, 416, 419, 423, 426, 430,
        433, 437, 440, 443, 447, 450, 453, 457, 460, 463, 467, 470, 473, 476, 480, 483, 486, 489,
        492, 496, 499, 502, 505, 508, 511, 515, 518, 521, 524, 527, 530, 533, 536, 539, 542, 545,
        548, 551, 554, 557, 560, 563, 566, 569, 572, 575, 578, 581, 584, 587, 590, 593, 596, 599,
        602, 605, 607, 610, 613, 616, 619, 622, 625, 627, 630, 633, 636, 639, 641, 644, 647, 650,
        653, 655, 658, 661, 664, 666, 669, 672, 675, 677, 680, 683, 685, 688, 691, 693, 696, 699,
        701, 704, 707, 709, 712, 715, 717, 720, 723, 725, 728, 730, 733, 736, 738, 741, 743, 746,
        749, 751, 754, 756, 759, 761, 764, 767, 769, 772, 774, 777, 779, 782, 784, 787, 789, 792,
        794, 797, 799, 802, 804, 807, 809, 812, 814, 817, 819, 821, 824, 826, 829, 831, 834, 836,
        838, 841, 843, 846, 848, 851, 853, 855, 858, 860, 863, 865, 867, 870, 872, 874, 877, 879,
        881, 884, 886, 889, 891, 893, 896, 898, 900, 903, 905, 907, 910, 912, 914, 916, 919, 921,
        923, 926, 928, 930, 933, 935, 937, 939, 942, 944, 946, 948, 951, 953, 955, 957, 960, 962,
        964, 966, 969, 971, 973, 975, 978, 980, 982, 984, 987, 989, 991, 993, 995, 998, 1000, 1002,
        1004, 1006, 1009, 1011, 1013, 1015, 1017, 1019, 1022, 1024, 1026, 1028, 1030, 1032, 1035,
        1037, 1039, 1041, 1043, 1045, 1048, 1050, 1052, 1054, 1056, 1058, 1060, 1063, 1065, 1067,
        1069, 1071, 1073, 1075, 1077, 1079, 1082, 1084, 1086, 1088, 1090, 1092, 1094, 1096, 1098,
        1100, 1103, 1105, 1107, 1109, 1111, 1113, 1115, 1117, 1119, 1121, 1123, 1125, 1127, 1129,
        1132, 1134, 1136, 1138, 1140, 1142, 1144, 1146, 1148, 1150, 1152, 1154, 1156, 1158, 1160,
        1162, 1164, 1166, 1168,
    ],
    [
        0, 11, 21, 30, 39, 47, 55, 63, 70, 78, 85, 92, 100, 106, 113, 120, 127, 133, 140, 146, 153,
        159, 165, 171, 177, 183, 189, 195, 201, 207, 213, 219, 224, 230, 236, 241, 247, 252, 258,
        263, 268, 274, 279, 284, 290, 295, 300, 305, 310, 316, 321, 326, 331, 336, 341, 346, 351,
        356, 360, 365, 370, 375, 380, 385, 389, 394, 399, 403, 408, 413, 417, 422, 427, 431, 436,
        440, 445, 449, 454, 458, 463, 467, 472, 476, 480, 485, 489, 493, 498, 502, 506, 511, 515,
        519, 523, 528, 532, 536, 540, 545, 549, 553, 557, 561, 565, 569, 573, 578, 582, 586, 590,
        594, 598, 602, 606, 610, 614, 618, 622, 626, 630, 633, 637, 641, 645, 649, 653, 657, 661,
        665, 668, 672, 676, 680, 684, 687, 691, 695, 699, 702, 706, 710, 714, 717, 721, 725, 729,
        732, 736, 740, 743, 747, 751, 754, 758, 761, 765, 769, 772, 776, 779, 783, 787, 790, 794,
        797, 801, 804, 808, 811, 815, 818, 822, 825, 829, 832, 836, 839, 843, 846, 850, 853, 856,
        860, 863, 867, 870, 873, 877, 880, 884, 887, 890, 894, 897, 900, 904, 907, 910, 914, 917,
        920, 924, 927, 930, 934, 937, 940, 943, 947, 950, 953, 957, 960, 963, 966, 969, 973, 976,
        979, 982, 986, 989, 992, 995, 998, 1002, 1005, 1008, 1011, 1014, 1017, 1021, 1024, 1027,
        1030, 1033, 1036, 1039, 1042, 1046, 1049, 1052, 1055, 1058, 1061, 1064, 1067, 1070, 1073,
        1076, 1080, 1083, 1086, 1089, 1092, 1095, 1098, 1101, 1104, 1107, 1110, 1113, 1116, 1119,
        1122, 1125, 1128, 1131, 1134, 1137, 1140, 1143, 1146, 1149, 1152, 1155, 1158, 1161, 1164,
        1167, 1170, 1172, 1175, 1178, 1181, 1184, 1187, 1190, 1193, 1196, 1199, 1202, 1205, 1207,
        1210, 1213, 1216, 1219, 1222, 1225, 1228, 1230, 1233, 1236, 1239, 1242, 1245, 1248, 1250,
        1253, 1256, 1259, 1262, 1265, 1267, 1270, 1273, 1276, 1279, 1281, 1284, 1287, 1290, 1293,
        1295, 1298, 1301, 1304, 1307, 1309, 1312, 1315, 1318, 1320, 1323, 1326, 1329, 1331, 1334,
        1337, 1340, 1342, 1345, 1348, 1351, 1353, 1356, 1359, 1361, 1364, 1367, 1370, 1372, 1375,
        1378, 1380, 1383, 1386, 1389, 1391, 1394, 1397, 1399, 1402, 1405, 1407, 1410, 1413, 1415,
        1418, 1421, 1423, 1426, 1429, 1431, 1434, 1436, 1439, 1442, 1444, 1447, 1450, 1452, 1455,
        1458, 1460, 1463,
    ],
];
// Squared gain in units of 2^-32, rounded to the nearest integer.
const ENERGY_WEIGHT: [u64; 209] = [
    0,
    1,
    1,
    2,
    2,
    2,
    3,
    3,
    4,
    5,
    6,
    7,
    8,
    10,
    11,
    13,
    16,
    19,
    23,
    27,
    32,
    38,
    45,
    54,
    64,
    76,
    91,
    108,
    128,
    152,
    181,
    215,
    256,
    304,
    362,
    431,
    512,
    609,
    724,
    861,
    1024,
    1218,
    1448,
    1722,
    2048,
    2435,
    2896,
    3444,
    4096,
    4871,
    5793,
    6889,
    8192,
    9742,
    11585,
    13777,
    16384,
    19484,
    23170,
    27554,
    32768,
    38968,
    46341,
    55109,
    65536,
    71468,
    77936,
    84990,
    92682,
    101070,
    110218,
    120194,
    131072,
    142935,
    155872,
    169979,
    185364,
    202141,
    220436,
    240387,
    262144,
    285870,
    311744,
    339959,
    370728,
    404281,
    440872,
    480774,
    524288,
    571740,
    623487,
    679917,
    741455,
    808563,
    881744,
    961548,
    1048576,
    1143480,
    1246974,
    1359835,
    1482910,
    1617125,
    1763488,
    1923097,
    2097152,
    2286960,
    2493948,
    2719670,
    2965821,
    3234251,
    3526975,
    3846194,
    4194304,
    4573921,
    4987896,
    5439339,
    5931642,
    6468501,
    7053950,
    7692387,
    8388608,
    9147842,
    9975792,
    10878679,
    11863283,
    12937002,
    14107901,
    15384775,
    16777216,
    18295684,
    19951585,
    21757357,
    23726566,
    25874004,
    28215802,
    30769550,
    33554432,
    36591368,
    39903169,
    43514715,
    47453133,
    51748008,
    56431603,
    61539100,
    67108864,
    73182735,
    79806339,
    87029429,
    94906266,
    103496017,
    112863206,
    123078199,
    134217728,
    146365470,
    159612677,
    174058859,
    189812531,
    206992033,
    225726413,
    246156398,
    268435456,
    292730940,
    319225354,
    348117717,
    379625062,
    413984066,
    451452825,
    492312797,
    536870912,
    585461881,
    638450708,
    696235434,
    759250125,
    827968132,
    902905651,
    984625594,
    1073741824,
    1170923762,
    1276901417,
    1392470869,
    1518500250,
    1655936265,
    1805811301,
    1969251188,
    2147483648,
    2341847524,
    2553802834,
    2784941738,
    3037000500,
    3311872529,
    3611622603,
    3938502376,
    4294967296,
    4683695048,
    5107605667,
    5569883475,
    6074001000,
    6623745059,
    7223245206,
    7877004752,
    8589934592,
    9367390096,
    10215211334,
    11139766951,
    12148002000,
    13247490117,
    14446490411,
    15754009503,
    17179869184,
];

fn table(dimensions: usize) -> &'static [u16; 388] {
    let index = match dimensions {
        20 => 0,
        40 => 1,
        80 => 2,
        160 => 3,
        320 => 4,
        400 => 5,
        800 => 6,
        _ => unreachable!("fixed band dimensions"),
    };
    &PULSE_COST[index]
}

#[cfg(test)]
fn cost(dimensions: usize, pulses: usize) -> usize {
    usize::from(table(dimensions)[pulses])
}

pub(crate) fn edges(samples: usize, bins: usize) -> [usize; 5] {
    [0, samples / 24, samples / 12, samples / 6, bins]
}

pub(crate) fn allocate(samples: usize, bins: usize, gains: &[u8; 4], bytes: usize) -> [usize; 4] {
    if *gains == [0; 4] {
        return [0; 4];
    }
    let edges = edges(samples, bins);
    let dimensions: [usize; 4] = std::array::from_fn(|i| edges[i + 1] - edges[i]);
    let tables = dimensions.map(table);
    let offsets = dimensions.map(|dimension| {
        let root = dimension.isqrt();
        root + usize::from(root * root != dimension)
    });
    let numerators: [u64; 4] =
        std::array::from_fn(|i| ENERGY_WEIGHT[usize::from(gains[i])] * dimensions[i] as u64);
    let mut pulses = gains.map(|code| usize::from(code != 0));
    let mut used = 36
        + (0..4)
            .map(|i| usize::from(tables[i][pulses[i]]))
            .sum::<usize>();
    // Only the selected band's marginal cost and priority can change.
    let next = |band: usize, pulses: usize| {
        if gains[band] == 0 || pulses == MAX_PULSES {
            (usize::MAX, 1u64)
        } else {
            let extra = usize::from(tables[band][pulses + 1] - tables[band][pulses]);
            let resolution = pulses + offsets[band];
            (extra, (resolution as u64).pow(3) * extra as u64)
        }
    };
    let mut priorities: [(usize, u64); 4] = std::array::from_fn(|band| next(band, pulses[band]));
    loop {
        let mut chosen = None;
        let mut best = (0u128, 1u128);
        for band in 0..4 {
            if priorities[band].0 > bytes * 8 - used {
                continue;
            }
            // Compare energy*N / ((K+ceil(sqrt(N)))^3 * added_bits) without division.
            let priority = (u128::from(numerators[band]), u128::from(priorities[band].1));
            if priority.0 * best.1 > best.0 * priority.1 {
                chosen = Some(band);
                best = priority;
            }
        }
        let Some(band) = chosen else {
            break;
        };
        pulses[band] += 1;
        used += priorities[band].0;
        priorities[band] = next(band, pulses[band]);
    }
    pulses
}

pub(crate) fn quantize(
    input: &[f32],
    pulses: usize,
    output: &mut [i16],
    indices: &mut [usize],
) -> f32 {
    output.fill(0);
    if pulses == 0 {
        return 0.0;
    }
    let sum: f32 = input.iter().map(|value| value.abs()).sum();
    for (index, value) in indices.iter_mut().enumerate() {
        *value = index;
    }
    let active = pulses.min(input.len());
    if active < input.len() {
        indices.select_nth_unstable_by(active, |&left, &right| {
            input[right]
                .abs()
                .total_cmp(&input[left].abs())
                .then(left.cmp(&right))
        });
    }
    let mut assigned = 0;
    let mut dot = 0.0;
    let mut norm = 0u32;
    for &index in &indices[..active] {
        let magnitude = if sum == 0.0 {
            0
        } else {
            ((input[index].abs() * pulses as f32 / sum).floor() as usize).min(pulses - assigned)
        };
        output[index] = if input[index] < 0.0 {
            -(magnitude as i16)
        } else {
            magnitude as i16
        };
        assigned += magnitude;
        norm += (magnitude * magnitude) as u32;
        dot += input[index].abs() * magnitude as f32;
    }
    while assigned < pulses {
        let mut selected = usize::MAX;
        let mut best = (0.0f32, 1u32);
        for &index in &indices[..active] {
            let magnitude = u32::from(output[index].unsigned_abs());
            let candidate = ((dot + input[index].abs()).powi(2), norm + 2 * magnitude + 1);
            let left = candidate.0 * best.1 as f32;
            let right = best.0 * candidate.1 as f32;
            if left > right || (left == right && index < selected) {
                selected = index;
                best = candidate;
            }
        }
        norm += 2 * u32::from(output[selected].unsigned_abs()) + 1;
        output[selected] += if input[selected] < 0.0 { -1 } else { 1 };
        dot += input[selected].abs();
        assigned += 1;
    }
    if pulses > 1 {
        norm = refine(input, output, &indices[..active], norm);
    }
    (norm as f32).sqrt()
}

fn refine(input: &[f32], output: &mut [i16], indices: &[usize], mut norm: u32) -> u32 {
    let mut dot: f64 = indices
        .iter()
        .map(|&i| f64::from(input[i].abs()) * f64::from(output[i].unsigned_abs()))
        .sum();
    // Each exchange preserves the pulse budget and must improve normalized correlation.
    for _ in 0..2 {
        let mut removed = usize::MAX;
        let mut best = (0.0f64, 1u32);
        for &index in indices {
            let magnitude = u32::from(output[index].unsigned_abs());
            if magnitude == 0 {
                continue;
            }
            let candidate = (
                dot - f64::from(input[index].abs()),
                norm - 2 * magnitude + 1,
            );
            let left = candidate.0.powi(2) * f64::from(best.1);
            let right = best.0.powi(2) * f64::from(candidate.1);
            if left > right || (left == right && index < removed) {
                removed = index;
                best = candidate;
            }
        }
        let mut added = usize::MAX;
        let mut improvement = (dot, norm);
        for &index in indices {
            if index == removed {
                continue;
            }
            let magnitude = u32::from(output[index].unsigned_abs());
            let candidate = (
                best.0 + f64::from(input[index].abs()),
                best.1 + 2 * magnitude + 1,
            );
            let left = candidate.0.powi(2) * f64::from(improvement.1);
            let right = improvement.0.powi(2) * f64::from(candidate.1);
            if left > right || (left == right && index < added && added != usize::MAX) {
                added = index;
                improvement = candidate;
            }
        }
        if added == usize::MAX {
            break;
        }
        output[removed] -= if input[removed] < 0.0 { -1 } else { 1 };
        output[added] += if input[added] < 0.0 { -1 } else { 1 };
        (dot, norm) = improvement;
    }
    norm
}

pub(crate) fn encode(
    writer: &mut entropy::Encoder<'_>,
    values: &[i16],
    pulses: usize,
) -> Result<(), entropy::Error> {
    if pulses == 0 {
        return Ok(());
    }
    let slots = values.len() + pulses - 1;
    let mut frequency = 0;
    let mut selected = 0;
    let mut in_bin = 0;
    for position in 0..slots {
        let remaining = pulses - selected;
        if remaining == 0 {
            break;
        }
        let available = slots - position;
        let present = in_bin < usize::from(values[frequency].unsigned_abs());
        if remaining != available {
            let probabilities = [0, (available - remaining) as u16, available as u16];
            writer.symbol(usize::from(present), Model::new(&probabilities)?)?;
        }
        if present {
            if in_bin == 0 {
                writer.raw(u32::from(values[frequency] < 0), 1)?;
            }
            selected += 1;
            in_bin += 1;
        } else {
            frequency += 1;
            in_bin = 0;
        }
    }
    Ok(())
}

pub(crate) fn decode(
    reader: &mut entropy::Decoder<'_>,
    values: &mut [i16],
    pulses: usize,
) -> Result<f32, entropy::Error> {
    values.fill(0);
    if pulses == 0 {
        return Ok(0.0);
    }
    let slots = values.len() + pulses - 1;
    let mut selected = 0;
    let mut norm = 0u32;
    for position in 0..slots {
        let remaining = pulses - selected;
        if remaining == 0 {
            break;
        }
        let available = slots - position;
        let present = if remaining == available {
            true
        } else {
            let probabilities = [0, (available - remaining) as u16, available as u16];
            reader.symbol(Model::new(&probabilities)?) != 0
        };
        if present {
            let frequency = position - selected;
            let value = values.get_mut(frequency).ok_or(entropy::Error::Corrupt)?;
            let sign = if *value == 0 {
                if reader.raw(1)? == 0 { 1 } else { -1 }
            } else if *value > 0 {
                1
            } else {
                -1
            };
            norm += 2 * u32::from(value.unsigned_abs()) + 1;
            *value += sign;
            selected += 1;
        }
    }
    Ok((norm as f32).sqrt())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pulse_costs_match_independent_combinatorial_logarithms() {
        for (table, &dimensions) in DIMENSIONS.iter().enumerate() {
            let mut combinations = 0.0f64;
            for (pulses, &bits) in PULSE_COST[table].iter().enumerate() {
                if pulses != 0 {
                    combinations += ((dimensions + pulses - 1) as f64 / pulses as f64).log2();
                }
                assert_eq!(usize::from(bits), combinations.ceil() as usize + pulses);
            }
        }
    }

    #[test]
    fn signed_multisets_roundtrip_inside_their_exact_budget() {
        for dimensions in DIMENSIONS {
            for pulses in [0, 1, 2, 7, 31, 166, MAX_PULSES] {
                let mut values = vec![0i16; dimensions];
                for i in 0..pulses {
                    let index = if i % 3 == 0 {
                        dimensions - 1
                    } else {
                        (i * 17) % dimensions
                    };
                    values[index] += if index % 2 == 0 { 1 } else { -1 };
                }
                let bytes = (cost(dimensions, pulses) + 4).div_ceil(8).max(1);
                let mut packet = vec![0; bytes];
                let mut writer = entropy::Encoder::new(&mut packet);
                encode(&mut writer, &values, pulses).unwrap();
                writer.finish_padded().unwrap();
                let mut reader = entropy::Decoder::new(&packet);
                let mut decoded = vec![0; dimensions];
                let norm = decode(&mut reader, &mut decoded, pulses).unwrap();
                assert_eq!(values, decoded);
                let expected = values
                    .iter()
                    .map(|&v| f64::from(v).powi(2))
                    .sum::<f64>()
                    .sqrt();
                assert!((f64::from(norm) - expected).abs() < 1e-4);
                assert!(reader.tell() <= (bytes * 8) as u64);
            }
        }
    }

    #[test]
    fn pulse_exchange_repairs_an_overfilled_frequency() {
        let input = [1.0, -0.6, 0.45];
        let mut values = [2, -2, 0];
        let norm = refine(&input, &mut values, &[0, 1, 2], 8);
        assert_eq!(values, [2, -1, 1]);
        assert_eq!(norm, 6);
        let dot: f64 = input
            .iter()
            .zip(values)
            .map(|(&x, q)| f64::from(x) * f64::from(q))
            .sum();
        assert!(dot * dot / f64::from(norm) > 3.2f64.powi(2) / 8.0);
    }

    #[test]
    fn pulse_exchange_preserves_budget_signs_and_normalized_correlation() {
        for dimensions in DIMENSIONS {
            let input: Vec<_> = (0..dimensions).map(|i| (i as f32 * 0.379).sin()).collect();
            let indices: Vec<_> = (0..dimensions).rev().collect();
            for pulses in [2, 7, 31, 166, MAX_PULSES] {
                for seed in 0..8 {
                    let mut values = vec![0i16; dimensions];
                    for i in 0..pulses {
                        let index = (i * (seed + 1) * 17 + seed) % dimensions;
                        values[index] += if input[index] < 0.0 { -1 } else { 1 };
                    }
                    let correlation = |values: &[i16]| {
                        let dot: f64 = input
                            .iter()
                            .zip(values)
                            .map(|(&x, &q)| f64::from(x) * f64::from(q))
                            .sum();
                        dot * dot / values.iter().map(|&q| f64::from(q).powi(2)).sum::<f64>()
                    };
                    let before = correlation(&values);
                    let original_norm: u32 = values
                        .iter()
                        .map(|&q| u32::from(q.unsigned_abs()).pow(2))
                        .sum();
                    let norm = refine(&input, &mut values, &indices, original_norm);
                    assert!(correlation(&values) + 1e-12 >= before);
                    assert_eq!(
                        values
                            .iter()
                            .map(|q| usize::from(q.unsigned_abs()))
                            .sum::<usize>(),
                        pulses
                    );
                    assert_eq!(
                        norm,
                        values
                            .iter()
                            .map(|&q| u32::from(q.unsigned_abs()).pow(2))
                            .sum::<u32>()
                    );
                    assert!(
                        input
                            .iter()
                            .zip(values)
                            .all(|(&x, q)| q == 0 || (x < 0.0) == (q < 0))
                    );
                }
            }
        }
        let mut values = [0, 2, 0];
        assert_eq!(refine(&[0.0; 3], &mut values, &[2, 1, 0], 4), 4);
        assert_eq!(values, [0, 2, 0]);
        let mut values = [-2];
        assert_eq!(refine(&[-1.0], &mut values, &[0], 4), 4);
        assert_eq!(values, [-2]);
        for indices in [[0, 1, 2, 3], [3, 2, 1, 0], [1, 3, 0, 2]] {
            let mut values = [2, 0, 0, 0];
            assert_eq!(refine(&[1.0; 4], &mut values, &indices, 4), 2);
            assert_eq!(values, [1, 1, 0, 0]);
        }
    }

    #[test]
    fn quantized_shapes_keep_their_pulse_sum_and_signs() {
        for dimensions in DIMENSIONS {
            let input: Vec<_> = (0..dimensions).map(|i| (i as f32 * 0.379).sin()).collect();
            let mut indices = vec![0; dimensions];
            let mut values = vec![0; dimensions];
            for pulses in [0, 1, 2, 7, 31, 166, MAX_PULSES] {
                let norm = quantize(&input, pulses, &mut values, &mut indices);
                assert_eq!(
                    values
                        .iter()
                        .map(|v| usize::from(v.unsigned_abs()))
                        .sum::<usize>(),
                    pulses
                );
                assert!(norm.is_finite());
                for (&x, &q) in input.iter().zip(&values) {
                    assert!(q == 0 || (x < 0.0) == (q < 0));
                }
            }
        }
    }

    #[test]
    fn allocation_respects_every_supported_packet_budget() {
        for samples in [480, 960] {
            for bins in [samples, samples / 2, samples / 3] {
                let edges = edges(samples, bins);
                for bytes in (samples / 32 - 4)..=(samples / 12 - 4) {
                    for gains in [[0; 4], [1; 4], [208; 4], [64, 96, 128, 192], [0, 208, 0, 1]] {
                        let pulses = allocate(samples, bins, &gains, bytes);
                        let used = 36
                            + (0..4)
                                .map(|i| cost(edges[i + 1] - edges[i], pulses[i]))
                                .sum::<usize>();
                        assert!(used <= bytes * 8);
                        for i in 0..4 {
                            if gains[i] != 0 && pulses[i] < MAX_PULSES {
                                assert!(
                                    used + cost(edges[i + 1] - edges[i], pulses[i] + 1)
                                        - cost(edges[i + 1] - edges[i], pulses[i])
                                        > bytes * 8
                                );
                            }
                        }
                    }
                }
            }
        }
    }
}
